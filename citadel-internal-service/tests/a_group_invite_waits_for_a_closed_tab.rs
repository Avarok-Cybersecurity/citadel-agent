//! A group invitation sent while the invitee has no tab open is still there when they come back.
//!
//! Found live, two Macs, 2026-09-28: the agent forwarded `GroupInviteNotification` only to the
//! localhost connection that owned the session at that moment. With the tab closed it went
//! nowhere, so the invitee opened the app later to find nothing. Joined groups already survived
//! (`GroupListJoined`); invitations did not. Now the session keeps them until they are answered,
//! and the same list returns them.
//!
//! Real: two agent sessions, a server, P2P, the group and the claim a returning tab makes.

use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::group::recv_until;
    use crate::common::{
        connect_p2p, open_localhost_connection, register_p2p, two_sessions_on_one_service_at,
    };
    use citadel_internal_service_types::{
        ConfigCommand, GroupCreateSuccess, GroupListJoinedSuccess, InternalServiceRequest,
        InternalServiceResponse, PendingGroupInvite,
    };
    use citadel_sdk::prelude::{MessageGroupKey, SessionSecuritySettingsBuilder, UserIdentifier};
    use std::error::Error;
    use std::time::Duration;
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
    use uuid::Uuid;

    type Tx = UnboundedSender<InternalServiceRequest>;
    type Rx = UnboundedReceiver<InternalServiceResponse>;

    async fn list_joined(tx: &Tx, rx: &mut Rx, cid: u64) -> GroupListJoinedSuccess {
        let request_id = Uuid::new_v4();
        tx.send(InternalServiceRequest::GroupListJoined { cid, request_id })
            .expect("service channel open");
        match recv_until(rx, "GroupListJoined answer", |r| match r {
            InternalServiceResponse::GroupListJoinedSuccess(s) => s.request_id == Some(request_id),
            InternalServiceResponse::GroupListJoinedFailure(f) => f.request_id == Some(request_id),
            _ => false,
        })
        .await
        {
            InternalServiceResponse::GroupListJoinedSuccess(s) => s,
            other => panic!("the joined groups could not be read: {other:?}"),
        }
    }

    async fn respond(
        tx: &Tx,
        rx: &mut Rx,
        cid: u64,
        peer_cid: u64,
        key: MessageGroupKey,
        accept: bool,
    ) {
        let request_id = Uuid::new_v4();
        tx.send(InternalServiceRequest::GroupRespondRequest {
            cid,
            peer_cid,
            group_key: key,
            response: accept,
            request_id,
            invitation: true,
        })
        .expect("service channel open");
        let answer = recv_until(rx, "GroupRespondRequest answer", |r| match r {
            InternalServiceResponse::GroupRespondRequestSuccess(s) => {
                s.request_id == Some(request_id)
            }
            InternalServiceResponse::GroupRespondRequestFailure(f) => {
                f.request_id == Some(request_id)
            }
            _ => false,
        })
        .await;
        assert!(
            matches!(
                answer,
                InternalServiceResponse::GroupRespondRequestSuccess(_)
            ),
            "the invitation could not be answered: {answer:?}"
        );
    }

    /// The owner invites the member while the member's tab is closed; a new tab claims the
    /// session. Returns that tab, the owner's cid, the member's cid and the group.
    async fn invited_while_away(
        tag: &str,
    ) -> Result<(Tx, Rx, u64, u64, MessageGroupKey), Box<dyn Error>> {
        crate::common::setup_log();
        let (
            addr,
            (mut owner_tx, mut owner_rx, owner_cid),
            (mut member_tx, mut member_rx, member_cid),
        ) = two_sessions_on_one_service_at(tag).await?;
        let settings = SessionSecuritySettingsBuilder::default().build()?;
        register_p2p(
            &mut owner_tx,
            &mut owner_rx,
            owner_cid,
            &mut member_tx,
            &mut member_rx,
            member_cid,
            settings,
            None,
        )
        .await?;
        connect_p2p(
            &mut owner_tx,
            &mut owner_rx,
            owner_cid,
            &mut member_tx,
            &mut member_rx,
            member_cid,
            settings,
            None,
        )
        .await?;

        // The member closes the tab. The session stays with the agent (kernel/ext.rs).
        drop(member_tx);
        drop(member_rx);
        tokio::time::sleep(Duration::from_millis(500)).await;

        owner_tx.send(InternalServiceRequest::GroupCreate {
            cid: owner_cid,
            request_id: Uuid::new_v4(),
            initial_users_to_invite: Some(vec![UserIdentifier::from(member_cid)]),
        })?;
        let InternalServiceResponse::GroupCreateSuccess(GroupCreateSuccess { group_key, .. }) =
            recv_until(&mut owner_rx, "GroupCreateSuccess", |r| {
                matches!(r, InternalServiceResponse::GroupCreateSuccess(..))
            })
            .await
        else {
            unreachable!()
        };
        // The invitation crosses the server to the member's session.
        tokio::time::sleep(Duration::from_secs(3)).await;

        // The member comes back: a new tab claims the orphaned session, as the landing chip does.
        let (tab_tx, mut tab_rx) = open_localhost_connection(addr).await?;
        let request_id = Uuid::new_v4();
        tab_tx.send(InternalServiceRequest::ConnectionManagement {
            request_id,
            management_command: ConfigCommand::ClaimSession {
                session_cid: member_cid,
                only_if_orphaned: true,
            },
        })?;
        let claimed = recv_until(&mut tab_rx, "ClaimSession answer", |r| match r {
            InternalServiceResponse::ConnectionManagementSuccess(s) => {
                s.request_id == Some(request_id)
            }
            InternalServiceResponse::ConnectionManagementFailure(f) => {
                f.request_id == Some(request_id)
            }
            _ => false,
        })
        .await;
        assert!(
            matches!(
                claimed,
                InternalServiceResponse::ConnectionManagementSuccess(_)
            ),
            "the returning tab could not claim the session: {claimed:?}"
        );
        Ok((tab_tx, tab_rx, owner_cid, member_cid, group_key))
    }

    #[tokio::test]
    async fn the_invitation_is_waiting_and_accepting_it_joins() -> Result<(), Box<dyn Error>> {
        let (tx, mut rx, owner_cid, member_cid, group_key) =
            invited_while_away("invitewaitaccept").await?;

        let listed = list_joined(&tx, &mut rx, member_cid).await;
        assert_eq!(
            listed.pending_invites,
            vec![PendingGroupInvite {
                peer_cid: owner_cid,
                group_key
            }]
        );
        assert!(
            listed.groups.is_empty(),
            "joined before answering: {:?}",
            listed.groups
        );

        respond(&tx, &mut rx, member_cid, owner_cid, group_key, true).await;
        let listed = list_joined(&tx, &mut rx, member_cid).await;
        assert_eq!(listed.groups, vec![group_key]);
        assert!(
            listed.pending_invites.is_empty(),
            "still offered after accepting"
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_declined_invitation_is_not_offered_again() -> Result<(), Box<dyn Error>> {
        let (tx, mut rx, owner_cid, member_cid, group_key) =
            invited_while_away("invitewaitdecline").await?;
        assert_eq!(
            list_joined(&tx, &mut rx, member_cid)
                .await
                .pending_invites
                .len(),
            1,
            "control: waiting"
        );

        respond(&tx, &mut rx, member_cid, owner_cid, group_key, false).await;
        let listed = list_joined(&tx, &mut rx, member_cid).await;
        assert!(
            listed.pending_invites.is_empty(),
            "still offered after declining"
        );
        assert!(listed.groups.is_empty());
        Ok(())
    }
}
