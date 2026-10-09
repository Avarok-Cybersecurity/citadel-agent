//! What to answer once the SDK disconnect has been attempted.
//!
//! Extracted so the decision can be tested without a live protocol stack. The
//! handler around it needs a real `NodeRemote`, an authenticated session and a
//! peer, so the branch that matters was reachable only by running the whole
//! agent -- which is why it went unexamined.
//!
//! The rule: the map entry is removed BEFORE the SDK is asked to disconnect, so
//! reporting success when the SDK refused or timed out leaves the caller wedged.
//! `connect` then finds no entry, calls `remote.connect()`, and the protocol
//! answers `SessionManagerSessionAlreadyExists`; `ClaimSession` and
//! `DisconnectOrphan` both answer "not found" because the entry is gone. No wire
//! command can reach the session that is still there, until the agent restarts.
//!
//! Both failure arms previously logged "Proceeding anyway" and fell through to
//! the success notification. `connection_management.rs` reports its equivalent
//! failure and says in its own comment that this file "has always done this
//! correctly -- so this is that fix, propagated". It was not: this file awaited
//! the result and discarded it. The fix was propagated from a file that never
//! had it, and the claim is why nobody re-read it.

use citadel_internal_service_types::{
    DisconnectNotification, InternalServiceResponse, PeerDisconnectFailure,
};
use citadel_sdk::prelude::SessionInfo;
use std::time::Duration;
use uuid::Uuid;

/// How the SDK's disconnect attempt ended.
#[derive(Debug)]
pub enum SdkDisconnect {
    /// The protocol session is gone.
    Succeeded,
    /// The SDK refused; carries its rendered error.
    Failed(String),
    /// The SDK did not answer within the budget.
    TimedOut,
    /// The server could not be told, so the session was ended on this device alone
    /// (`abandon_session`); carries why the server was not told. Signed out.
    EndedLocally(String),
}

/// Whether the SDK still holds `cid` as a session that is up. A session whose every connection
/// has been marked inactive is ending (the SDK does that when it processes a disconnect, well
/// before the session object is dropped), so it is not live.
pub fn session_is_live(sessions: &[SessionInfo], cid: u64) -> bool {
    sessions
        .iter()
        .find(|session| session.cid == cid)
        .is_some_and(|session| session.connections.iter().any(|conn| conn.connected))
}

impl SdkDisconnect {
    /// Decides by state, not by the error text: a failure (or timeout) while the SDK no longer
    /// holds the session live means the sign-out happened, whatever the error said. `live` is
    /// `None` when the SDK could not be asked, and then the failure stands.
    pub fn settle(self, live: Option<bool>) -> Self {
        match (self.needs_local_end(), live) {
            (true, Some(false)) => SdkDisconnect::Succeeded,
            _ => self,
        }
    }

    /// The outcome once an SDK failure has been followed by abandoning the session locally.
    /// A refused or timed-out disconnect becomes `EndedLocally` when the abandon worked; when
    /// it did not, the session survives and the failure stands, with the abandon's error added.
    /// An outcome that was not a failure is returned unchanged.
    pub fn after_abandon(self, abandoned: Result<(), String>, budget: Duration) -> Self {
        let why = match &self {
            SdkDisconnect::Failed(err) => format!("SDK disconnect failed: {err}"),
            SdkDisconnect::TimedOut => format!("SDK disconnect timed out after {budget:?}"),
            SdkDisconnect::Succeeded | SdkDisconnect::EndedLocally(_) => return self,
        };
        match abandoned {
            Ok(()) => SdkDisconnect::EndedLocally(why),
            Err(abandon_err) => SdkDisconnect::Failed(format!(
                "{why}; ending the session locally failed: {abandon_err}"
            )),
        }
    }

    /// Whether the SDK failed in a way the session survived, so it still has to be ended here.
    pub fn needs_local_end(&self) -> bool {
        matches!(self, SdkDisconnect::Failed(_) | SdkDisconnect::TimedOut)
    }

    /// The response to send. Success only when the session actually went away.
    pub fn into_response(
        self,
        cid: u64,
        peer_cid: Option<u64>,
        request_id: Uuid,
        budget: Duration,
    ) -> InternalServiceResponse {
        let message = match self {
            SdkDisconnect::Succeeded => {
                return InternalServiceResponse::DisconnectNotification(DisconnectNotification {
                    cid,
                    peer_cid,
                    request_id: Some(request_id),
                    ended_locally: None,
                })
            }
            SdkDisconnect::EndedLocally(why) => {
                return InternalServiceResponse::DisconnectNotification(DisconnectNotification {
                    cid,
                    peer_cid,
                    request_id: Some(request_id),
                    ended_locally: Some(why),
                })
            }
            SdkDisconnect::Failed(err) => format!("SDK disconnect failed: {err}"),
            SdkDisconnect::TimedOut => {
                format!("SDK disconnect timed out after {budget:?}")
            }
        };

        InternalServiceResponse::PeerDisconnectFailure(PeerDisconnectFailure {
            cid,
            message,
            request_id: Some(request_id),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUDGET: Duration = Duration::from_secs(10);

    fn response_of(outcome: SdkDisconnect) -> InternalServiceResponse {
        outcome.into_response(7, Some(9), Uuid::nil(), BUDGET)
    }

    fn info(cid: u64, connected: &[bool]) -> SessionInfo {
        use citadel_sdk::prelude::{ConnectionInfo, VirtualConnectionType};
        SessionInfo {
            cid,
            connections: connected
                .iter()
                .map(|connected| ConnectionInfo {
                    peer_cid: None,
                    adjacent_nat_type: None,
                    connection_type: VirtualConnectionType::LocalGroupServer { session_cid: cid },
                    connected: *connected,
                    latest_ratchet_version: 0,
                })
                .collect(),
        }
    }

    #[test]
    fn only_a_session_with_a_connection_up_is_live() {
        assert!(session_is_live(&[info(1, &[true]), info(2, &[])], 1));
        assert!(!session_is_live(&[info(1, &[false])], 1), "ending");
        assert!(!session_is_live(&[info(2, &[])], 2), "nothing up");
        assert!(!session_is_live(&[info(1, &[true])], 3), "absent");
    }

    #[test]
    fn a_disconnect_error_while_the_session_is_ending_is_a_sign_out() {
        // The interleaving of the field failure: the SDK's disconnect errors because the
        // session is still listed, the abandon then fails "not an active session", and by the
        // time either is looked at the session is ending.
        let err = "C2S session 1 still in protocol after disconnect".to_string();
        let abandoned = Err("not an active session".to_string());
        let ending = [info(1, &[false])];
        let outcome = SdkDisconnect::Failed(err)
            .after_abandon(abandoned, BUDGET)
            .settle(Some(session_is_live(&ending, 1)));
        match response_of(outcome) {
            InternalServiceResponse::DisconnectNotification(n) => assert_eq!(n.cid, 7),
            other => panic!("an ended session is signed out: {other:?}"),
        }
    }

    #[test]
    fn a_session_still_live_after_the_abandon_stays_a_failure() {
        // The negative control: the same failures, but the session is up.
        for failure in [SdkDisconnect::Failed("x".into()), SdkDisconnect::TimedOut] {
            let live = [info(1, &[true])];
            let outcome = failure
                .after_abandon(Err("not an active session".into()), BUDGET)
                .settle(Some(session_is_live(&live, 1)));
            assert!(matches!(
                response_of(outcome),
                InternalServiceResponse::PeerDisconnectFailure(_)
            ));
        }
    }

    #[test]
    fn a_state_that_could_not_be_read_does_not_turn_a_failure_into_a_sign_out() {
        let outcome = SdkDisconnect::TimedOut.settle(None);
        assert!(matches!(
            response_of(outcome),
            InternalServiceResponse::PeerDisconnectFailure(_)
        ));
    }

    #[test]
    fn a_refused_disconnect_is_not_reported_as_a_sign_out() {
        // The defect: this returned DisconnectNotification, so the UI signed the
        // user out while the protocol session survived -- and the map entry was
        // already gone, so nothing could reach it afterwards.
        match response_of(SdkDisconnect::Failed("channel closed".into())) {
            InternalServiceResponse::PeerDisconnectFailure(f) => {
                assert_eq!(f.cid, 7);
                assert!(
                    f.message.contains("channel closed"),
                    "the reason must survive into the message: {}",
                    f.message
                );
            }
            other => panic!("a refused disconnect must not report success: {other:?}"),
        }
    }

    #[test]
    fn a_timed_out_disconnect_is_not_reported_as_a_sign_out() {
        match response_of(SdkDisconnect::TimedOut) {
            InternalServiceResponse::PeerDisconnectFailure(f) => {
                assert!(
                    f.message.contains("timed out"),
                    "the caller should be able to tell a timeout from a refusal: {}",
                    f.message
                );
            }
            other => panic!("a timed-out disconnect must not report success: {other:?}"),
        }
    }

    #[test]
    fn a_real_disconnect_is_still_reported_as_one() {
        // The control. A decision that answered PeerDisconnectFailure for every
        // outcome would satisfy both assertions above and break sign-out
        // entirely -- the UI would never see the notification it waits for.
        match response_of(SdkDisconnect::Succeeded) {
            InternalServiceResponse::DisconnectNotification(n) => {
                assert_eq!(n.cid, 7);
                assert_eq!(n.peer_cid, Some(9));
            }
            other => panic!("a successful disconnect must report success: {other:?}"),
        }
    }

    #[test]
    fn a_failure_followed_by_a_local_end_is_a_sign_out_that_says_so() {
        for failure in [
            SdkDisconnect::Failed("link down".into()),
            SdkDisconnect::TimedOut,
        ] {
            let outcome = failure.after_abandon(Ok(()), BUDGET);
            match response_of(outcome) {
                InternalServiceResponse::DisconnectNotification(n) => {
                    let note = n
                        .ended_locally
                        .expect("the note says the server was not told");
                    assert!(!note.is_empty());
                }
                other => panic!("an abandoned session is signed out: {other:?}"),
            }
        }
    }

    #[test]
    fn a_failed_local_end_keeps_the_failure() {
        let outcome = SdkDisconnect::TimedOut.after_abandon(Err("no session".into()), BUDGET);
        match response_of(outcome) {
            InternalServiceResponse::PeerDisconnectFailure(f) => {
                assert!(f.message.contains("timed out") && f.message.contains("no session"));
            }
            other => panic!("a session that survived is not signed out: {other:?}"),
        }
    }

    #[test]
    fn a_success_is_not_marked_as_ended_locally() {
        let outcome = SdkDisconnect::Succeeded.after_abandon(Ok(()), BUDGET);
        match response_of(outcome) {
            InternalServiceResponse::DisconnectNotification(n) => assert_eq!(n.ended_locally, None),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_request_id_is_carried_on_every_outcome() {
        // Without it the caller cannot correlate the answer with its request,
        // and a failure that nobody can attribute is close to a failure nobody
        // sees.
        for outcome in [
            SdkDisconnect::Succeeded,
            SdkDisconnect::Failed("x".into()),
            SdkDisconnect::TimedOut,
            SdkDisconnect::EndedLocally("x".into()),
        ] {
            let id = match outcome.into_response(1, None, Uuid::nil(), BUDGET) {
                InternalServiceResponse::DisconnectNotification(n) => n.request_id,
                InternalServiceResponse::PeerDisconnectFailure(f) => f.request_id,
                other => panic!("unexpected variant: {other:?}"),
            };
            assert_eq!(id, Some(Uuid::nil()));
        }
    }
}
