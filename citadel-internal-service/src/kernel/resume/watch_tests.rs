//! The watch against a clock a test controls: it fires at the moment, once, and a computer
//! that slept through the moment or had its clock moved is found out at the next look.

use super::expiry::RECHECK;
use super::test_support::{credential_accepted_at, ISSUED_AT};
use super::watch::{deadline, sign_in_notice, until_expired, Clock};
use citadel_internal_service_types::NoticeKind;
use futures::future::BoxFuture;
use parking_lot::Mutex;
use std::time::Duration;

const START: u64 = 1_700_000_000_000;
const HOUR: u64 = 3_600_000;

/// A clock whose every sleep moves it by what the script says, which need not be what was
/// asked: a suspended computer wakes hours after a thirty-second sleep began.
struct Scripted {
    now: Mutex<u64>,
    /// What each sleep really took, in order; a sleep past the script takes what was asked.
    took: Mutex<Vec<i64>>,
    asked: Mutex<Vec<Duration>>,
}

impl Scripted {
    fn new(took: &[i64]) -> Self {
        Self {
            now: Mutex::new(START),
            took: Mutex::new(took.iter().rev().copied().collect()),
            asked: Mutex::new(Vec::new()),
        }
    }

    fn sleeps(&self) -> usize {
        self.asked.lock().len()
    }
}

impl Clock for Scripted {
    fn now_ms(&self) -> u64 {
        *self.now.lock()
    }

    fn sleep(&self, duration: Duration) -> BoxFuture<'_, ()> {
        self.asked.lock().push(duration);
        // A watch that never reaches its moment fails here instead of spinning for ever.
        assert!(self.sleeps() < 10_000, "the watch is not getting anywhere");
        let took = self.took.lock().pop();
        let took = took.unwrap_or(duration.as_millis() as i64);
        let mut now = self.now.lock();
        *now = now.saturating_add_signed(took);
        Box::pin(async {})
    }
}

async fn fired(clock: &Scripted, expires_at: u64) -> bool {
    until_expired(clock, expires_at, || true).await
}

/// Control: return from `until_expired` after its first sleep.
#[tokio::test]
async fn it_fires_at_the_moment_with_no_drop_and_no_sooner() {
    let clock = Scripted::new(&[]);
    assert!(fired(&clock, START + 95_000).await);
    assert_eq!(clock.now_ms(), START + 95_000, "it fired early or late");
    assert_eq!(
        *clock.asked.lock(),
        [RECHECK, RECHECK, RECHECK, Duration::from_secs(5)]
    );
}

/// Control: sleep for the whole of what is left, in one sleep.
#[tokio::test]
async fn a_computer_that_slept_through_the_moment_fires_when_it_wakes() {
    // An hour is left; the first thirty-second sleep is a suspend of five hours.
    let clock = Scripted::new(&[5 * HOUR as i64]);
    assert!(fired(&clock, START + HOUR).await);
    assert_eq!(
        clock.sleeps(),
        1,
        "it slept again after waking past the moment"
    );
}

/// Control: count sleeps instead of reading the clock.
#[tokio::test]
async fn a_clock_set_back_waits_again_and_one_set_forward_fires_at_once() {
    // Two minutes left. After one look the clock is set back ten minutes.
    let back = Scripted::new(&[-600_000]);
    assert!(fired(&back, START + 120_000).await);
    assert_eq!(back.now_ms(), START + 120_000);
    assert!(back.sleeps() > 20, "it fired on a count of sleeps");

    let forward = Scripted::new(&[HOUR as i64]);
    assert!(fired(&forward, START + 120_000).await);
    assert_eq!(forward.sleeps(), 1);
}

/// Control: ignore `still_held`.
#[tokio::test]
async fn a_session_that_no_longer_holds_the_credential_is_told_nothing() {
    let clock = Scripted::new(&[]);
    let looks = Mutex::new(0);
    let held_twice = || {
        let mut looks = looks.lock();
        *looks += 1;
        *looks <= 2
    };
    assert!(!until_expired(&clock, START + HOUR, held_twice).await);
    assert_eq!(clock.sleeps(), 2);
    let already_expired = until_expired(&clock, START, || false).await;
    assert!(!already_expired, "a session that is gone was told");
}

/// Control: make `deadline` the server's `valid_until_utc` instead of `expires_at_local`.
#[tokio::test]
async fn a_device_whose_clock_is_wrong_waits_for_the_moment_on_its_own_clock() {
    // An hour's credential, accepted on a computer three hours ahead, then three hours behind.
    for skew in [3 * HOUR as i64, -3 * HOUR as i64] {
        let local = ISSUED_AT.saturating_add_signed(skew);
        let held = credential_accepted_at(3_600, local).await;
        assert_eq!(
            held.valid_until_utc(),
            ISSUED_AT + HOUR,
            "the server's time"
        );
        assert_eq!(
            deadline(&held),
            local + HOUR,
            "the moment by this computer's clock, {skew} ms off the server's"
        );
    }
}

#[test]
fn the_notice_opens_the_accounts_sign_in() {
    let notice = sign_in_notice(7, "alice".into(), Some("acme.example".into()));
    assert_eq!(notice.kind, NoticeKind::SignInNeeded);
    assert_eq!(notice.target.account, "alice");
    assert_eq!(notice.target.server_host.as_deref(), Some("acme.example"));
    assert_eq!(
        notice.target.open, "sign-in",
        "the page's link target for the sign-in"
    );
    assert!(notice.title.contains("alice"));
}
