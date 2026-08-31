//! Bounding the wait for a membership response.
//!
//! Split out of `respond_request.rs`, which was over the 250-line cap once the
//! bound and its tests were added. Keeping the wait here also keeps it
//! testable: the handler needs a live `CitadelWorkspaceService`, and this does
//! not.

use citadel_sdk::prelude::{
    GroupBroadcast, GroupChannel, GroupChannelCreated, GroupEvent, NodeResult, Ratchet,
};
use futures::{Stream, StreamExt};
use std::time::Duration;

/// How long to wait for the protocol's answer to a membership response.
///
/// The answer to an AcceptMembership broadcast comes from the group OWNER's
/// node, not from the server alone, so an offline owner produces no event at
/// all. Thirty seconds matches `DEREGISTER_WAIT` in `deregister.rs` — the
/// crate's convention for bounding a callback subscription whose answer may
/// never come. The alternative to a bound is a handler that never answers,
/// which reaches the user as an invitation spinner that never clears.
pub(super) const GROUP_RESPOND_WAIT: Duration = Duration::from_secs(30);

/// The terminal event of a membership response, separated from the side
/// effects the handler performs on it so the bounded wait is testable
/// without a running node.
pub(super) enum InvitationOutcome {
    /// The group channel opened — the accept succeeded.
    ChannelCreated(GroupChannel),
    /// The protocol answered with an explicit success flag.
    MembershipAnswered(bool),
    /// The kernel dropped the subscription without a terminal event.
    Ended,
    /// Nothing arrived within `GROUP_RESPOND_WAIT` — the owner is likely
    /// offline.
    TimedOut,
}

/// Drives the callback subscription to the invitation's terminal event,
/// giving up after `wait`.
///
/// This used to be an unbounded `subscription.next()` loop in `handle`: when
/// the owner was offline the loop never advanced, so the localhost request
/// never resolved and the spawned handler task leaked for the life of the
/// process. The bound converts that silence into a failure response.
pub(super) async fn await_invitation_outcome<R: Ratchet, S>(
    events: &mut S,
    wait: Duration,
) -> InvitationOutcome
where
    S: Stream<Item = NodeResult<R>> + Unpin,
{
    let outcome = tokio::time::timeout(wait, async {
        while let Some(evt) = events.next().await {
            match evt {
                // When accepting an invite, we expect a GroupChannelCreated in response
                NodeResult::GroupChannelCreated(GroupChannelCreated { channel, .. }) => {
                    return Some(InvitationOutcome::ChannelCreated(channel));
                }
                NodeResult::GroupEvent(GroupEvent {
                    event: GroupBroadcast::AcceptMembershipResponse { key: _, success },
                    ..
                })
                | NodeResult::GroupEvent(GroupEvent {
                    event: GroupBroadcast::DeclineMembershipResponse { key: _, success },
                    ..
                }) => return Some(InvitationOutcome::MembershipAnswered(success)),
                _ => {}
            }
        }
        None
    })
    .await;

    match outcome {
        Ok(Some(outcome)) => outcome,
        Ok(None) => InvitationOutcome::Ended,
        Err(_elapsed) => InvitationOutcome::TimedOut,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use citadel_sdk::prelude::{MessageGroupKey, StackedRatchet, Ticket};

    fn membership_response(accept: bool, success: bool) -> NodeResult<StackedRatchet> {
        let key = MessageGroupKey::new(1, 1);
        let event = if accept {
            GroupBroadcast::AcceptMembershipResponse { key, success }
        } else {
            GroupBroadcast::DeclineMembershipResponse { key, success }
        };
        NodeResult::GroupEvent(GroupEvent {
            session_cid: 1,
            ticket: Ticket(1),
            event,
        })
    }

    /// A non-terminal event the wait must skip over, not mistake for the
    /// answer: the outbound request form, not a response.
    fn non_terminal_event() -> NodeResult<StackedRatchet> {
        NodeResult::GroupEvent(GroupEvent {
            session_cid: 1,
            ticket: Ticket(1),
            event: GroupBroadcast::AcceptMembership {
                target: 1,
                key: MessageGroupKey::new(1, 1),
            },
        })
    }

    /// The M2 defect: an offline group owner never answers, and the wait used
    /// to be unbounded, hanging the request forever. The outer timeout is the
    /// test's own guard — with the unbounded loop reintroduced, the inner
    /// future never resolves and this test fails at the `expect`.
    #[tokio::test]
    async fn unanswered_subscription_times_out_instead_of_hanging() {
        let mut events = futures::stream::pending::<NodeResult<StackedRatchet>>();
        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            await_invitation_outcome(&mut events, Duration::from_millis(100)),
        )
        .await
        .expect("the wait must resolve on its own; hanging forever is the defect");
        assert!(matches!(outcome, InvitationOutcome::TimedOut));
    }

    /// The explicit success flag from the protocol is surfaced verbatim, and
    /// non-terminal events before it are skipped rather than consumed as the
    /// answer.
    #[tokio::test]
    async fn membership_answers_are_surfaced() {
        for (accept, success) in [(true, true), (true, false), (false, true)] {
            let mut events = futures::stream::iter(vec![
                non_terminal_event(),
                membership_response(accept, success),
            ]);
            let outcome = await_invitation_outcome(&mut events, Duration::from_secs(5)).await;
            match outcome {
                InvitationOutcome::MembershipAnswered(answered) => assert_eq!(answered, success),
                _ => panic!("expected MembershipAnswered"),
            }
        }
    }

    /// A subscription the kernel drops without answering is a failure, not a
    /// hang and not a success.
    #[tokio::test]
    async fn ended_subscription_is_not_an_answer() {
        let mut events = futures::stream::iter(Vec::<NodeResult<StackedRatchet>>::new());
        let outcome = await_invitation_outcome(&mut events, Duration::from_secs(5)).await;
        assert!(matches!(outcome, InvitationOutcome::Ended));
    }
}
