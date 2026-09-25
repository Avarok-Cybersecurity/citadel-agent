//! A group member whose session moves to another localhost connection must go
//! on receiving the group.
//!
//! The move is driven by the exact request sequence the UI's "Use it here"
//! takeover sends (TakeoverSignIn -> login-with-password -> claim-session.ts):
//! a password `Connect` answered `SessionAlreadyActive`, then
//! `ClaimSession { only_if_orphaned: true }` (refused "not orphaned"), then
//! `ClaimSession { only_if_orphaned: false }` (accepted: the Connect already
//! re-pointed the session here).
//!
//! These pass on the tree they were written against: the Connect re-points the
//! session's shared `associated_localhost_connection` and the group receiver's
//! `SessionRoute` resolves through it on every send. They go red if the route
//! is frozen at spawn (checked by making `SessionRoute::new` copy the uuid into
//! a fresh atomic), which is the regression they guard.

use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::group::{
        group_message_arrives, joined_group, joined_group_on_one_service, recv_until,
        send_group_message, JoinedGroup, OneServiceGroup,
    };
    use crate::common::open_localhost_connection;
    use citadel_internal_service_types::{
        ConfigCommand, InternalServiceRequest, InternalServiceResponse,
    };
    use std::error::Error;
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
    use uuid::Uuid;

    /// B's credentials as `services_connected_to_one_server` registers them.
    const MEMBER_USERNAME: &str = "peer.1";
    const MEMBER_PASSWORD: &[u8] = b"secret_1";

    type Tx = UnboundedSender<InternalServiceRequest>;
    type Rx = UnboundedReceiver<InternalServiceResponse>;

    fn connect_request(
        request_id: Uuid,
        username: &str,
        password: &[u8],
    ) -> InternalServiceRequest {
        InternalServiceRequest::Connect {
            username: username.to_string(),
            password: password.to_vec().into(),
            connect_mode: Default::default(),
            udp_mode: Default::default(),
            keep_alive_timeout: None,
            session_security_settings: Default::default(),
            request_id,
            server_password: None,
        }
    }

    async fn claim(
        tx: &Tx,
        rx: &mut Rx,
        cid: u64,
        only_if_orphaned: bool,
    ) -> InternalServiceResponse {
        let request_id = Uuid::new_v4();
        tx.send(InternalServiceRequest::ConnectionManagement {
            request_id,
            management_command: ConfigCommand::ClaimSession {
                session_cid: cid,
                only_if_orphaned,
            },
        })
        .expect("service channel open");
        recv_until(rx, "ClaimSession answer", |r| match r {
            InternalServiceResponse::ConnectionManagementSuccess(s) => {
                s.request_id == Some(request_id)
            }
            InternalServiceResponse::ConnectionManagementFailure(f) => {
                f.request_id == Some(request_id)
            }
            _ => false,
        })
        .await
    }

    /// The UI's takeover, request for request.
    async fn take_over(tx: &Tx, rx: &mut Rx, cid: u64) {
        take_over_as(tx, rx, cid, MEMBER_USERNAME, MEMBER_PASSWORD).await
    }

    async fn take_over_as(tx: &Tx, rx: &mut Rx, cid: u64, username: &str, password: &[u8]) {
        let request_id = Uuid::new_v4();
        tx.send(connect_request(request_id, username, password))
            .expect("channel open");
        let answer = recv_until(rx, "takeover Connect answer", |r| match r {
            InternalServiceResponse::SessionAlreadyActive(a) => a.request_id == Some(request_id),
            InternalServiceResponse::ConnectSuccess(s) => s.request_id == Some(request_id),
            InternalServiceResponse::ConnectFailure(f) => f.request_id == Some(request_id),
            _ => false,
        })
        .await;
        let InternalServiceResponse::SessionAlreadyActive(active) = answer else {
            panic!("the takeover Connect was not answered SessionAlreadyActive: {answer:?}");
        };
        assert_eq!(
            active.cid, cid,
            "SessionAlreadyActive named another session"
        );

        match claim(tx, rx, cid, true).await {
            InternalServiceResponse::ConnectionManagementFailure(f) => assert!(
                f.error.contains("not orphaned"),
                "claim-session.ts reads 'not orphaned'; got {}",
                f.error
            ),
            other => panic!("a live session was claimable as an orphan: {other:?}"),
        }
        match claim(tx, rx, cid, false).await {
            InternalServiceResponse::ConnectionManagementSuccess(_) => {}
            other => panic!("the session was not held by the new connection: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_member_keeps_receiving_the_group_after_its_session_moves(
    ) -> Result<(), Box<dyn Error>> {
        let JoinedGroup {
            to_service_a,
            from_service_a: _from_service_a,
            cid_a,
            to_service_b: _old_connection_tx,
            from_service_b: mut old_connection,
            cid_b,
            group_key,
            service_b_addr,
        } = joined_group().await?;

        // Control: delivery works before the move, so a silence afterwards is
        // the move's doing and not a group that never delivered.
        send_group_message(&to_service_a, cid_a, group_key, b"before the move");
        let before = group_message_arrives(&mut old_connection, b"before the move")
            .await
            .ok_or("control: the member never received the group before the move")?;
        assert_eq!(before.cid, cid_b);

        // A second browser window, and "Use it here". The first window stays open.
        let (new_tx, mut new_connection) = open_localhost_connection(service_b_addr).await?;
        take_over(&new_tx, &mut new_connection, cid_b).await;

        send_group_message(&to_service_a, cid_a, group_key, b"after the move");
        let after = group_message_arrives(&mut new_connection, b"after the move")
            .await
            .ok_or(
                "the member's new connection never received the owner's group message \
                 after the session moved to it",
            )?;
        assert_eq!(after.cid, cid_b, "delivered, but for the wrong session");
        assert_eq!(
            after.peer_cid, cid_a,
            "delivered, but from the wrong sender"
        );
        Ok(())
    }

    /// Both accounts on ONE agent, as in the live report (alice and bob0924
    /// were both sessions of the agent on :12345).
    #[tokio::test]
    async fn same_agent_member_keeps_receiving_after_its_session_moves(
    ) -> Result<(), Box<dyn Error>> {
        let tag = "grouptakeover";
        let OneServiceGroup {
            service_addr,
            owner: (owner_tx, mut owner_rx, owner_cid),
            member: (_member_tx, mut member_rx, member_cid),
            group_key,
        } = joined_group_on_one_service(tag).await?;

        send_group_message(&owner_tx, owner_cid, group_key, b"before the move");
        group_message_arrives(&mut member_rx, b"before the move")
            .await
            .ok_or("control: the member never received the group before the move")?;

        let (new_tx, mut new_connection) = open_localhost_connection(service_addr).await?;
        take_over_as(
            &new_tx,
            &mut new_connection,
            member_cid,
            &format!("{tag}.1"),
            b"secret_1",
        )
        .await;

        // What the UI does next on the new connection (reconcile-groups.ts on
        // cid change, and a PeerConnect to the DM partner).
        for (tx, rx, cid) in [
            (&new_tx, &mut new_connection, member_cid),
            (&owner_tx, &mut owner_rx, owner_cid),
        ] {
            let request_id = Uuid::new_v4();
            tx.send(InternalServiceRequest::GroupListGroupsFor {
                cid,
                peer_cid: None,
                request_id,
            })?;
            let listed = recv_until(rx, "GroupListGroupsFor answer", |r| match r {
                InternalServiceResponse::GroupListGroupsSuccess(s) => {
                    s.request_id == Some(request_id)
                }
                InternalServiceResponse::GroupListGroupsFailure(f) => {
                    f.request_id == Some(request_id)
                }
                _ => false,
            })
            .await;
            assert!(
                matches!(listed, InternalServiceResponse::GroupListGroupsSuccess(_)),
                "the group list could not be read for {cid}: {listed:?}"
            );
        }

        send_group_message(&owner_tx, owner_cid, group_key, b"after the move");
        group_message_arrives(&mut new_connection, b"after the move")
            .await
            .ok_or("the new connection never received the group after the move")?;
        Ok(())
    }
}
