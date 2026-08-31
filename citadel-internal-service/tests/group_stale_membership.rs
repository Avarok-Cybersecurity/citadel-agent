use citadel_internal_service_test_common as common;

/// Pins the M5 fix: `Connection.groups` used to be insert-only, so after a
/// `GroupLeave` the stale map entry still satisfied the membership check in
/// `requests/group/message.rs` and a `GroupMessage` to the departed group was
/// answered with `GroupMessageSuccess`. This test leaves a group and then
/// asserts the very next `GroupMessage` for it is answered with
/// `GroupMessageFailure`. It fails (receives success) without the departed-flag
/// mechanism in `src/kernel/group_channels.rs`.
#[cfg(test)]
mod tests {
    use crate::common::*;
    use citadel_internal_service_types::{
        GroupCreateSuccess, GroupInviteNotification, GroupLeaveNotification, GroupLeaveSuccess,
        GroupRespondRequestSuccess, InternalServiceRequest, InternalServiceResponse,
    };
    use citadel_sdk::logging::info;
    use citadel_sdk::prelude::{MessageGroupKey, StackedRatchet, UserIdentifier};
    use std::error::Error;
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
    use uuid::Uuid;

    /// Drains a service's response stream until `pred` matches, skipping the
    /// unrelated notifications (member-state changes, channel-created acks)
    /// that interleave nondeterministically with the responses under test.
    async fn recv_until(
        rx: &mut UnboundedReceiver<InternalServiceResponse>,
        what: &str,
        pred: impl Fn(&InternalServiceResponse) -> bool,
    ) -> InternalServiceResponse {
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let response = rx.recv().await.expect("service stream ended");
                if pred(&response) {
                    return response;
                }
                info!(target: "citadel", "[recv_until:{what}] skipping {response:?}");
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
    }

    /// A live group with A as owner and B as an accepted member.
    ///
    /// Extracted so the two departure routes -- B leaving, and the group
    /// being ended out from under B -- are tested against the same starting
    /// point rather than two hand-copied ones that could drift.
    struct JoinedGroup {
        to_service_a: UnboundedSender<InternalServiceRequest>,
        from_service_a: UnboundedReceiver<InternalServiceResponse>,
        cid_a: u64,
        to_service_b: UnboundedSender<InternalServiceRequest>,
        from_service_b: UnboundedReceiver<InternalServiceResponse>,
        cid_b: u64,
        group_key: MessageGroupKey,
    }

    async fn joined_group() -> Result<JoinedGroup, Box<dyn Error>> {
        crate::common::setup_log();
        let bind_address_internal_service_a: SocketAddr =
            format!("127.0.0.1:{}", get_free_port()).parse().unwrap();
        let bind_address_internal_service_b: SocketAddr =
            format!("127.0.0.1:{}", get_free_port()).parse().unwrap();

        let mut peer_return_handle_vec =
            register_and_connect_to_server_then_peers::<StackedRatchet>(
                vec![
                    bind_address_internal_service_a,
                    bind_address_internal_service_b,
                ],
                None,
                None,
            )
            .await?;

        let (to_service_a, mut from_service_a, cid_a) =
            peer_return_handle_vec.take_next_service_handle();
        let (to_service_b, mut from_service_b, cid_b) =
            peer_return_handle_vec.take_next_service_handle();

        // A creates a group, inviting B
        to_service_a.send(InternalServiceRequest::GroupCreate {
            cid: cid_a,
            request_id: Uuid::new_v4(),
            initial_users_to_invite: Some(vec![UserIdentifier::from(cid_b)]),
        })?;
        let create_response = recv_until(&mut from_service_a, "GroupCreateSuccess", |r| {
            matches!(r, InternalServiceResponse::GroupCreateSuccess(..))
        })
        .await;
        let InternalServiceResponse::GroupCreateSuccess(GroupCreateSuccess { group_key, .. }) =
            create_response
        else {
            unreachable!()
        };

        // B accepts the invitation
        let invite = recv_until(&mut from_service_b, "GroupInviteNotification", |r| {
            matches!(r, InternalServiceResponse::GroupInviteNotification(..))
        })
        .await;
        let InternalServiceResponse::GroupInviteNotification(GroupInviteNotification {
            peer_cid,
            group_key: invited_key,
            ..
        }) = invite
        else {
            unreachable!()
        };
        assert_eq!(invited_key, group_key);
        to_service_b.send(InternalServiceRequest::GroupRespondRequest {
            cid: cid_b,
            peer_cid,
            group_key,
            response: true,
            request_id: Uuid::new_v4(),
            invitation: true,
        })?;
        let accept = recv_until(&mut from_service_b, "GroupRespondRequest response", |r| {
            matches!(
                r,
                InternalServiceResponse::GroupRespondRequestSuccess(..)
                    | InternalServiceResponse::GroupRespondRequestFailure(..)
            )
        })
        .await;
        let InternalServiceResponse::GroupRespondRequestSuccess(GroupRespondRequestSuccess {
            ..
        }) = accept
        else {
            panic!("B failed to accept the group invitation: {accept:?}")
        };
        Ok(JoinedGroup {
            to_service_a,
            from_service_a,
            cid_a,
            to_service_b,
            from_service_b,
            cid_b,
            group_key,
        })
    }

    #[tokio::test]
    async fn group_message_after_leave_is_rejected() -> Result<(), Box<dyn Error>> {
        let JoinedGroup {
            to_service_a: _to_service_a,
            from_service_a: _from_service_a,
            cid_a: _cid_a,
            to_service_b,
            mut from_service_b,
            cid_b,
            group_key,
        } = joined_group().await?;

        // Membership sanity check: while still a member, B's GroupMessage must
        // succeed — otherwise the assertion below would also pass for the
        // wrong reason (a group that never worked at all).
        let while_member_id = Uuid::new_v4();
        to_service_b.send(InternalServiceRequest::GroupMessage {
            cid: cid_b,
            message: b"hello while still a member".to_vec(),
            group_key,
            request_id: while_member_id,
        })?;
        let while_member = recv_until(&mut from_service_b, "GroupMessage (member) response", |r| {
            matches!(
                r,
                InternalServiceResponse::GroupMessageSuccess(s) if s.request_id == Some(while_member_id)
            ) || matches!(
                r,
                InternalServiceResponse::GroupMessageFailure(f) if f.request_id == Some(while_member_id)
            )
        })
        .await;
        assert!(
            matches!(
                while_member,
                InternalServiceResponse::GroupMessageSuccess(..)
            ),
            "control: messaging while a member must succeed, got {while_member:?}"
        );

        // B leaves the group
        let leave_id = Uuid::new_v4();
        to_service_b.send(InternalServiceRequest::GroupLeave {
            cid: cid_b,
            group_key,
            request_id: leave_id,
        })?;
        let leave = recv_until(&mut from_service_b, "GroupLeave response", |r| {
            matches!(
                r,
                InternalServiceResponse::GroupLeaveSuccess(s) if s.request_id == Some(leave_id)
            ) || matches!(
                r,
                InternalServiceResponse::GroupLeaveFailure(f) if f.request_id == Some(leave_id)
            )
        })
        .await;
        let InternalServiceResponse::GroupLeaveSuccess(GroupLeaveSuccess { .. }) = leave else {
            panic!("B failed to leave the group: {leave:?}")
        };
        let leave_notification = recv_until(&mut from_service_b, "GroupLeaveNotification", |r| {
            matches!(r, InternalServiceResponse::GroupLeaveNotification(..))
        })
        .await;
        let InternalServiceResponse::GroupLeaveNotification(GroupLeaveNotification {
            success, ..
        }) = leave_notification
        else {
            unreachable!()
        };
        assert!(success, "server rejected B's leave");

        // THE PROPERTY UNDER TEST: a GroupMessage to the departed group must
        // be rejected. With the insert-only groups map this was answered with
        // GroupMessageSuccess from the stale entry.
        let after_leave_id = Uuid::new_v4();
        to_service_b.send(InternalServiceRequest::GroupMessage {
            cid: cid_b,
            message: b"hello after leaving".to_vec(),
            group_key,
            request_id: after_leave_id,
        })?;
        let after_leave = recv_until(&mut from_service_b, "GroupMessage (departed) response", |r| {
            matches!(
                r,
                InternalServiceResponse::GroupMessageSuccess(s) if s.request_id == Some(after_leave_id)
            ) || matches!(
                r,
                InternalServiceResponse::GroupMessageFailure(f) if f.request_id == Some(after_leave_id)
            )
        })
        .await;
        assert!(
            matches!(after_leave, InternalServiceResponse::GroupMessageFailure(..)),
            "GroupMessage after leaving the group must fail, but the service answered {after_leave:?}"
        );

        Ok(())
    }

    /// The other departure route, and the one `leave()` cannot cover: the group
    /// is ended out from under B.
    ///
    /// `leave()`/`end()` mark the entry departed only for departures THIS
    /// session initiates. Being removed by somebody else arrives as a server
    /// event (`EndResponse` / `Disconnected`), and until those marked the entry
    /// too, B's channel outlived the group — so a `GroupMessage` into a group
    /// that no longer existed was still answered with success, because the SDK
    /// send half merely enqueues into the session request queue and that
    /// succeeds forever.
    #[tokio::test]
    async fn group_message_after_the_group_ends_is_rejected() -> Result<(), Box<dyn Error>> {
        let JoinedGroup {
            to_service_a,
            from_service_a: _from_service_a,
            cid_a,
            to_service_b,
            mut from_service_b,
            cid_b,
            group_key,
        } = joined_group().await?;

        // Same sanity check as the sibling test: B must be able to send WHILE a
        // member, or the assertion below would pass for the wrong reason.
        let while_member_id = Uuid::new_v4();
        to_service_b.send(InternalServiceRequest::GroupMessage {
            cid: cid_b,
            message: b"still a member".to_vec(),
            group_key,
            request_id: while_member_id,
        })?;
        let while_member = recv_until(&mut from_service_b, "GroupMessage (member) response", |r| {
            matches!(
                r,
                InternalServiceResponse::GroupMessageSuccess(s) if s.request_id == Some(while_member_id)
            ) || matches!(
                r,
                InternalServiceResponse::GroupMessageFailure(f) if f.request_id == Some(while_member_id)
            )
        })
        .await;
        assert!(
            matches!(
                while_member,
                InternalServiceResponse::GroupMessageSuccess(..)
            ),
            "B could not message the group while still a member, so this test \
             cannot tell a departed group from a broken one: {while_member:?}"
        );

        // A ends the group. B never asked for anything.
        to_service_a.send(InternalServiceRequest::GroupEnd {
            cid: cid_a,
            group_key,
            request_id: Uuid::new_v4(),
        })?;

        // Wait for B to be TOLD, rather than sleeping: the notification is the
        // event that must also have marked the entry departed.
        let _ended = recv_until(
            &mut from_service_b,
            "GroupEnd/Disconnect notification",
            |r| {
                matches!(
                    r,
                    InternalServiceResponse::GroupEndNotification(..)
                        | InternalServiceResponse::GroupDisconnectNotification(..)
                )
            },
        )
        .await;

        let after_id = Uuid::new_v4();
        to_service_b.send(InternalServiceRequest::GroupMessage {
            cid: cid_b,
            message: b"after the group ended".to_vec(),
            group_key,
            request_id: after_id,
        })?;
        let after = recv_until(&mut from_service_b, "GroupMessage (ended) response", |r| {
            matches!(
                r,
                InternalServiceResponse::GroupMessageSuccess(s) if s.request_id == Some(after_id)
            ) || matches!(
                r,
                InternalServiceResponse::GroupMessageFailure(f) if f.request_id == Some(after_id)
            )
        })
        .await;
        assert!(
            matches!(after, InternalServiceResponse::GroupMessageFailure(..)),
            "GroupMessage after the group ended must fail, but the service \
             answered {after:?}"
        );

        Ok(())
    }
}
