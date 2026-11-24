//! When a session's resume credential runs out, decided from two readings of this
//! computer's clock and nothing else. Pure.
//!
//! The wait is never longer than [`RECHECK`]: a computer that slept through the moment, or
//! whose clock was set forward or back, is found out at the next look instead of at the end
//! of one long sleep that the pause stretched. Nothing here asks the server anything.

use std::time::Duration;

/// The longest the watch sleeps before it reads the clock again.
pub(crate) const RECHECK: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// Not yet: look again after this long.
    Wait(Duration),
    /// The credential has expired: say so.
    Expired,
}

/// What to do when the clock reads `now_ms` and the credential expires at `expires_at_ms`
/// (both Unix milliseconds, by this computer's clock).
pub(crate) fn step(now_ms: u64, expires_at_ms: u64) -> Step {
    match expires_at_ms.checked_sub(now_ms) {
        None | Some(0) => Step::Expired,
        Some(left) => Step::Wait(Duration::from_millis(left).min(RECHECK)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: u64 = 1_700_000_000_000;

    /// Control: wait while `now == expires_at`.
    #[test]
    fn it_expires_at_the_moment_and_not_a_millisecond_before() {
        assert_eq!(step(AT - 1, AT), Step::Wait(Duration::from_millis(1)));
        assert_eq!(step(AT, AT), Step::Expired);
        assert_eq!(step(AT + 1, AT), Step::Expired);
        assert_eq!(step(u64::MAX, AT), Step::Expired);
    }

    /// Control: wait for the whole of what is left.
    #[test]
    fn it_never_sleeps_past_the_next_look_at_the_clock() {
        let hour = 3_600_000;
        assert_eq!(step(AT - hour, AT), Step::Wait(RECHECK));
        assert_eq!(step(AT - 30_001, AT), Step::Wait(RECHECK));
        assert_eq!(
            step(AT - 29_999, AT),
            Step::Wait(Duration::from_millis(29_999))
        );
    }
}
