//! A member whose P2P connection to a group's owner was ACCEPTED, not dialled,
//! can still find and join that owner's group.
//!
//! The agent answers a connect for an account it hosts, and keeps only the
//! channel: an acceptor-side peer connection carries no PeerRemote. Joining a
//! group and listing a peer's groups both required that remote, so every
//! acceptor was refused before any request left ("Peer connection missing
//! remote (acceptor-only connection)"). An app topic's joiner then saw
//! `Unavailable("topic")` whenever the topic's owner was the one who dialled.
//!
//! Over the real SDK: two agents on one in-process server. `B` is hosted, so
//! its agent answers `A`'s connect; `A` owns the group.
use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::agent_ilm::declare_agent_ilm;
    use crate::common::setup_log;
    use crate::common::turn_harness::{next_matching, two_registered_agents};
    use crate::common::PeerReturnHandle;
    use citadel_internal_service_types::{InternalServiceRequest, InternalServiceResponse};
    use citadel_sdk::prelude::*;
    use std::error::Error;
    use uuid::Uuid;

    fn split(agents: &mut [PeerReturnHandle]) -> (&mut PeerReturnHandle, &mut PeerReturnHandle) {
        let (a, b) = agents.split_at_mut(1);
        (&mut a[0], &mut b[0])
    }

    /// `A` dials, `B`'s agent answers: `B` holds a channel-only connection to `A`.
    async fn a_dials_hosted_b(a: &mut PeerReturnHandle, b: &mut PeerReturnHandle) {
        b.0.send(declare_agent_ilm()).unwrap();
        next_matching(&mut b.1, "AgentCapabilities", |r| {
            matches!(r, InternalServiceResponse::AgentCapabilities(_)).then_some(())
        })
        .await;
        a.0.send(InternalServiceRequest::PeerConnect {
            request_id: Uuid::new_v4(),
            cid: a.2,
            peer_cid: b.2,
            udp_mode: UdpMode::Disabled,
            session_security_settings: SessionSecuritySettingsBuilder::default().build().unwrap(),
            peer_session_password: None,
            turn: None,
        })
        .unwrap();
        let answered = next_matching(&mut b.1, "PeerConnectNotification", |r| match r {
            InternalServiceResponse::PeerConnectNotification(n) => Some(n.answered_by_agent),
            _ => None,
        })
        .await;
        assert!(answered, "B's agent must answer, or B is not the acceptor");
        for (rx, peer) in [(&mut a.1, b.2), (&mut b.1, a.2)] {
            next_matching(rx, "PeerConnectSuccess", |r| match r {
                InternalServiceResponse::PeerConnectSuccess(s) => {
                    (s.peer_cid == peer).then_some(())
                }
                _ => None,
            })
            .await;
        }
    }

    async fn a_creates_a_group(a: &mut PeerReturnHandle) -> MessageGroupKey {
        a.0.send(InternalServiceRequest::GroupCreate {
            cid: a.2,
            request_id: Uuid::new_v4(),
            initial_users_to_invite: None,
        })
        .unwrap();
        next_matching(&mut a.1, "GroupCreateSuccess", |r| match r {
            InternalServiceResponse::GroupCreateSuccess(s) => Some(s.group_key),
            _ => None,
        })
        .await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_acceptor_joins_the_dialling_owners_group() -> Result<(), Box<dyn Error>> {
        setup_log();
        let mut agents = two_registered_agents().await?;
        let (a, b) = split(&mut agents);
        a_dials_hosted_b(a, b).await;
        let key = a_creates_a_group(a).await;

        b.0.send(InternalServiceRequest::GroupRequestJoin {
            cid: b.2,
            group_key: key,
            request_id: Uuid::new_v4(),
        })?;
        // The ask reaches the owner: it never did, the join was refused locally.
        let asked = next_matching(&mut a.1, "GroupJoinRequestNotification", |r| match r {
            InternalServiceResponse::GroupJoinRequestNotification(n) => Some(n),
            _ => None,
        })
        .await;
        assert_eq!((asked.peer_cid, asked.group_key), (b.2, key));
        a.0.send(InternalServiceRequest::GroupRespondRequest {
            cid: a.2,
            peer_cid: b.2,
            group_key: key,
            response: true,
            request_id: Uuid::new_v4(),
            invitation: false,
        })?;
        let joined = next_matching(&mut b.1, "GroupRequestJoin answer", |r| match r {
            InternalServiceResponse::GroupRequestJoinSuccess(s) => Some(Ok(s.group_key)),
            InternalServiceResponse::GroupRequestJoinFailure(f) => Some(Err(f.message)),
            _ => None,
        })
        .await;
        assert_eq!(joined, Ok(key));
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_acceptor_lists_the_dialling_owners_groups() -> Result<(), Box<dyn Error>> {
        setup_log();
        let mut agents = two_registered_agents().await?;
        let (a, b) = split(&mut agents);
        a_dials_hosted_b(a, b).await;
        let key = a_creates_a_group(a).await;

        b.0.send(InternalServiceRequest::GroupListGroupsFor {
            cid: b.2,
            peer_cid: Some(a.2),
            request_id: Uuid::new_v4(),
        })?;
        let listed = next_matching(&mut b.1, "GroupListGroupsFor answer", |r| match r {
            InternalServiceResponse::GroupListGroupsSuccess(s) => Some(Ok(s.group_list)),
            InternalServiceResponse::GroupListGroupsFailure(f) => Some(Err(f.message)),
            _ => None,
        })
        .await;
        assert_eq!(listed, Ok(Some(vec![key])));
        Ok(())
    }
}
