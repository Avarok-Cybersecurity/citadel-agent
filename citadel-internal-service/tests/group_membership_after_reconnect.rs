//! A group member whose C2S session is re-established -- `Disconnect`, then a
//! fresh `Connect` on another localhost connection -- while the group lives on
//! at the server.
//!
//! The new SDK session is not a working member. Group messages are end-to-end
//! encrypted under a CGKA state held by the SDK session
//! (`state_container.group_cgka`, citadel_proto `rekey_and_groups.rs`), and the
//! client drops a broadcast it has no state for or cannot decrypt
//! (`packet_processor/peer/group_broadcast.rs`, the client arm of
//! `GroupBroadcast::Message`). A new session starts with neither that state nor a
//! group channel, and the owner's `GroupMessage` is still answered with success,
//! so the loss is silent at both ends. There is no member-side rejoin through
//! the server: `GroupListGroupsFor` is `list_owned_groups`, keyed by the owner,
//! so the member cannot even list what it belonged to, and `GroupRequestJoin`
//! needs a live P2P remote to the owner.
//!
//! What does work, and what this pins: the OWNER re-inviting the member, who
//! accepts, which rebuilds both the member's CGKA state and its channel.

use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::group::{
        group_message_arrives, joined_group_on_one_service, recv_until, send_group_message,
        OneServiceGroup,
    };
    use crate::common::open_localhost_connection;
    use citadel_internal_service_types::{InternalServiceRequest, InternalServiceResponse};
    use std::error::Error;
    use uuid::Uuid;

    #[tokio::test]
    async fn a_reinvite_restores_delivery_after_the_members_session_is_reestablished(
    ) -> Result<(), Box<dyn Error>> {
        let tag = "groupreconnect";
        let OneServiceGroup {
            service_addr,
            owner: (owner_tx, mut owner_rx, owner_cid),
            member: (member_tx, mut member_rx, member_cid),
            group_key,
        } = joined_group_on_one_service(tag).await?;

        send_group_message(&owner_tx, owner_cid, group_key, b"before");
        group_message_arrives(&mut member_rx, b"before")
            .await
            .ok_or("control: no delivery before the reconnect")?;

        member_tx.send(InternalServiceRequest::Disconnect {
            request_id: Uuid::new_v4(),
            cid: member_cid,
        })?;
        let ended = recv_until(&mut member_rx, "Disconnect answer", |r| {
            matches!(
                r,
                InternalServiceResponse::DisconnectNotification(_)
                    | InternalServiceResponse::DisconnectFailure(_)
            )
        })
        .await;
        assert!(
            matches!(ended, InternalServiceResponse::DisconnectNotification(_)),
            "the member could not end its session: {ended:?}"
        );

        let (new_tx, mut new_rx) = open_localhost_connection(service_addr).await?;
        let connect_id = Uuid::new_v4();
        new_tx.send(InternalServiceRequest::Connect {
            username: format!("{tag}.1"),
            password: b"secret_1".to_vec().into(),
            connect_mode: Default::default(),
            udp_mode: Default::default(),
            keep_alive_timeout: None,
            session_security_settings: Default::default(),
            request_id: connect_id,
            server_password: None,
        })?;
        let connected = recv_until(&mut new_rx, "Connect answer", |r| match r {
            InternalServiceResponse::ConnectSuccess(s) => s.request_id == Some(connect_id),
            InternalServiceResponse::ConnectFailure(f) => f.request_id == Some(connect_id),
            InternalServiceResponse::SessionAlreadyActive(a) => a.request_id == Some(connect_id),
            _ => false,
        })
        .await;
        let InternalServiceResponse::ConnectSuccess(success) = connected else {
            panic!("the member could not sign in again: {connected:?}");
        };
        assert_eq!(success.cid, member_cid, "a CID is permanent per account");

        let invite_id = Uuid::new_v4();
        owner_tx.send(InternalServiceRequest::GroupInvite {
            cid: owner_cid,
            peer_cid: member_cid,
            group_key,
            request_id: invite_id,
        })?;
        let invited = recv_until(&mut owner_rx, "GroupInvite answer", |r| match r {
            InternalServiceResponse::GroupInviteSuccess(s) => s.request_id == Some(invite_id),
            InternalServiceResponse::GroupInviteFailure(f) => f.request_id == Some(invite_id),
            _ => false,
        })
        .await;
        assert!(
            matches!(invited, InternalServiceResponse::GroupInviteSuccess(_)),
            "the owner could not re-invite the member: {invited:?}"
        );

        let invitation = recv_until(&mut new_rx, "GroupInviteNotification", |r| {
            matches!(r, InternalServiceResponse::GroupInviteNotification(n) if n.group_key == group_key)
        })
        .await;
        let InternalServiceResponse::GroupInviteNotification(invitation) = invitation else {
            unreachable!()
        };
        let accept_id = Uuid::new_v4();
        new_tx.send(InternalServiceRequest::GroupRespondRequest {
            cid: member_cid,
            peer_cid: invitation.peer_cid,
            group_key,
            response: true,
            request_id: accept_id,
            invitation: true,
        })?;
        let accepted = recv_until(&mut new_rx, "GroupRespondRequest answer", |r| match r {
            InternalServiceResponse::GroupRespondRequestSuccess(s) => {
                s.request_id == Some(accept_id)
            }
            InternalServiceResponse::GroupRespondRequestFailure(f) => {
                f.request_id == Some(accept_id)
            }
            _ => false,
        })
        .await;
        assert!(
            matches!(
                accepted,
                InternalServiceResponse::GroupRespondRequestSuccess(_)
            ),
            "the member could not accept the re-invite: {accepted:?}"
        );

        send_group_message(&owner_tx, owner_cid, group_key, b"after the rejoin");
        let after = group_message_arrives(&mut new_rx, b"after the rejoin")
            .await
            .ok_or("the re-invited member's new session never received the group")?;
        assert_eq!(after.cid, member_cid);
        Ok(())
    }
}
