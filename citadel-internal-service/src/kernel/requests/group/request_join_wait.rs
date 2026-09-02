//! Bounding the wait for the answer to a join request.
//!
//! `request_join.rs` broke its `subscription.next()` loop only on
//! `RequestJoinPending`. The server answers the SAME ticket with two other
//! variants — `GroupNonExists` when the group is gone (the owner ended it
//! between the client listing it and asking to join), and
//! `AcceptMembershipResponse { success: true }` when the group is auto-accept
//! and the joiner was admitted outright. Both were dropped on the floor and the
//! loop re-awaited a subscription that never ends: its only sender lives in the
//! SDK callback map until the subscription drops, and the subscription is owned
//! by the hung handler. One leaked task and one leaked callback entry per
//! request, and a caller that receives neither success nor failure.
//!
//! This is the same defect `respond_wait.rs` was written for, in the sibling
//! handler, and it is the same fix: name every terminal event, and bound the
//! wait so silence becomes a failure response rather than a spinner that never
//! clears.

use citadel_sdk::prelude::{GroupBroadcast, GroupEvent, NodeResult, Ratchet};
use futures::{Stream, StreamExt};
use std::time::Duration;

/// How long to wait for the protocol's answer to a join request.
///
/// Matches `GROUP_RESPOND_WAIT` and `DEREGISTER_WAIT` — the crate's convention
/// for bounding a callback subscription whose answer may never come.
pub(super) const GROUP_REQUEST_JOIN_WAIT: Duration = Duration::from_secs(30);

/// The terminal events of a join request, separated from the handler's response
/// mapping so the wait is testable without a running node.
pub(super) enum JoinOutcome {
    /// The owner was asked; `Ok` means the request is pending their answer.
    Pending(Result<(), String>),
    /// The group is auto-accept and the joiner was admitted (or refused) at once.
    Answered(bool),
    /// The group does not exist — the owner ended it, or the key was never valid.
    GroupGone,
    /// The kernel dropped the subscription without a terminal event.
    Ended,
    /// Nothing arrived within `wait`.
    TimedOut,
}

/// Drives the callback subscription to the join request's terminal event,
/// giving up after `wait`.
pub(super) async fn await_join_outcome<R: Ratchet, S>(events: &mut S, wait: Duration) -> JoinOutcome
where
    S: Stream<Item = NodeResult<R>> + Unpin,
{
    let outcome = tokio::time::timeout(wait, async {
        while let Some(evt) = events.next().await {
            if let NodeResult::GroupEvent(GroupEvent { event, .. }) = evt {
                match event {
                    GroupBroadcast::RequestJoinPending { result, key: _ } => {
                        return Some(JoinOutcome::Pending(result));
                    }
                    // The auto-accept path: peer_layer.request_join returned
                    // Some(true) and the server admitted the joiner outright.
                    GroupBroadcast::AcceptMembershipResponse { key: _, success } => {
                        return Some(JoinOutcome::Answered(success));
                    }
                    // message_group_exists was false. GroupEnd removes the
                    // group, so this is an ordinary race, not a malformed
                    // request.
                    GroupBroadcast::GroupNonExists { key: _ } => {
                        return Some(JoinOutcome::GroupGone);
                    }
                    _ => {}
                }
            }
        }
        None
    })
    .await;

    match outcome {
        Ok(Some(outcome)) => outcome,
        Ok(None) => JoinOutcome::Ended,
        Err(_elapsed) => JoinOutcome::TimedOut,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use citadel_sdk::prelude::{MessageGroupKey, StackedRatchet, Ticket};

    fn event(event: GroupBroadcast) -> NodeResult<StackedRatchet> {
        NodeResult::GroupEvent(GroupEvent {
            session_cid: 1,
            ticket: Ticket(1),
            event,
        })
    }

    fn key() -> MessageGroupKey {
        MessageGroupKey::new(1, 1)
    }

    /// A non-terminal event the wait must skip rather than mistake for an
    /// answer: the outbound request form, not a response to it.
    fn non_terminal() -> NodeResult<StackedRatchet> {
        event(GroupBroadcast::RequestJoin {
            sender: 1,
            key: key(),
        })
    }

    /// The defect: an answer the loop did not name left it awaiting a
    /// subscription that never ends. The outer timeout is the test's own guard.
    #[tokio::test]
    async fn an_unanswered_request_times_out_instead_of_hanging() {
        let mut events = futures::stream::pending::<NodeResult<StackedRatchet>>();
        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            await_join_outcome(&mut events, Duration::from_millis(100)),
        )
        .await
        .expect("the wait must resolve on its own; hanging forever is the defect");
        assert!(matches!(outcome, JoinOutcome::TimedOut));
    }

    /// The two variants the handler used to discard. Each arrives on the same
    /// ticket as the request, so each reached the loop and was ignored.
    #[tokio::test]
    async fn the_variants_the_loop_used_to_drop_are_terminal() {
        let mut gone = futures::stream::iter(vec![
            non_terminal(),
            event(GroupBroadcast::GroupNonExists { key: key() }),
        ]);
        assert!(matches!(
            await_join_outcome(&mut gone, Duration::from_secs(5)).await,
            JoinOutcome::GroupGone
        ));

        for success in [true, false] {
            let mut auto = futures::stream::iter(vec![
                non_terminal(),
                event(GroupBroadcast::AcceptMembershipResponse {
                    key: key(),
                    success,
                }),
            ]);
            match await_join_outcome(&mut auto, Duration::from_secs(5)).await {
                JoinOutcome::Answered(answered) => assert_eq!(answered, success),
                _ => panic!("expected Answered"),
            }
        }
    }

    /// The variant that always worked, kept as the control: the fix must widen
    /// the match rather than replace it.
    #[tokio::test]
    async fn request_join_pending_is_still_the_ordinary_answer() {
        for result in [Ok(()), Err("owner refused".to_string())] {
            let expected = result.clone();
            let mut events = futures::stream::iter(vec![
                non_terminal(),
                event(GroupBroadcast::RequestJoinPending {
                    result: result.clone(),
                    key: key(),
                }),
            ]);
            match await_join_outcome(&mut events, Duration::from_secs(5)).await {
                JoinOutcome::Pending(got) => assert_eq!(got, expected),
                _ => panic!("expected Pending"),
            }
        }
    }

    /// A subscription the kernel drops without answering is a failure, not a
    /// hang and not a success.
    #[tokio::test]
    async fn an_ended_subscription_is_not_an_answer() {
        let mut events = futures::stream::iter(Vec::<NodeResult<StackedRatchet>>::new());
        assert!(matches!(
            await_join_outcome(&mut events, Duration::from_secs(5)).await,
            JoinOutcome::Ended
        ));
    }
}
