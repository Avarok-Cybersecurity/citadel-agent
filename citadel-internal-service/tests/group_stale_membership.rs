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
    use crate::common::group::{joined_group, recv_until, JoinedGroup};
    use citadel_internal_service_types::{
        GroupLeaveNotification, GroupLeaveSuccess, InternalServiceRequest, InternalServiceResponse,
    };
    use std::error::Error;
    use uuid::Uuid;

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
            ..
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
            ..
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
