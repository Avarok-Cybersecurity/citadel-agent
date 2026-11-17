//! P12/P13 at the host: a proof names the origin it is handed, a wrong password signs nothing
//! and counts toward the lockout, a connection without an origin is refused, fields out of
//! bounds are refused, and signing leaves the lock state as it was.
//!
//! Control: drop the `wrong_password` call in `sign_challenge` → the request after a wrong
//! password is admitted and `a_wrong_password_signs_nothing_and_starts_the_limit` goes red.
//! Control: admit at `Instant::now()` instead of `self.clock.now()` → the request past the
//! interval on the host's clock is refused and
//! `a_second_request_inside_the_interval_is_rate_limited` goes red.

use super::*;
use crate::kernel::identity::test_ports::{host, host_and_clock, host_with, SHIPPED_SIGN_LIMITS};
use crate::kernel::identity::SignLimits;
use citadel_identity::{base64url_decode, verify_proof};
use citadel_internal_service_types::IdentityState;
use std::time::Duration;

const ISSUER: &str = "work.avarok.net";
const ORIGIN: &str = "https://work.avarok.net";
const AUD: &str = "sw-mcp";

fn secret(text: &str) -> Zeroizing<Vec<u8>> {
    Zeroizing::new(text.as_bytes().to_vec())
}

fn request() -> SignRequest {
    SignRequest {
        issuer: ISSUER.into(),
        aud: AUD.into(),
        nonce: vec![0x30; 32],
        challenge: vec![0xda; 32],
    }
}

fn reason(result: Result<IdentityOutcome, Refused>) -> IdentityRefusal {
    result.expect_err("refused").0
}

#[tokio::test]
async fn a_locked_identity_signs_after_the_re_proof_and_stays_locked() {
    let host = host();
    let IdentityOutcome::Created { pk, .. } = host.create(secret("pw")).await.unwrap() else {
        panic!("not created")
    };
    host.unlocked.lock();
    let conn = Uuid::new_v4();
    let outcome = host
        .sign_challenge(conn, Some(ORIGIN.into()), request(), secret("pw"))
        .await
        .unwrap();
    let IdentityOutcome::SignedChallenge { proof } = outcome else {
        panic!("{outcome:?}")
    };
    let blob = base64url_decode(&proof).unwrap();
    let proved = verify_proof(&blob, ISSUER, ORIGIN, AUD, &[0x30; 32], &[0xda; 32]).unwrap();
    assert_eq!(
        proved.identity.to_base64url(),
        pk,
        "the subject is this identity"
    );
    assert!(matches!(
        host.status().await.unwrap(),
        IdentityOutcome::Status {
            state: IdentityState::Locked { .. }
        }
    ));
}

#[tokio::test]
async fn a_wrong_password_signs_nothing_and_starts_the_limit() {
    // A limit of one wrong password and no interval, so the second request shows the lockout
    // without five Argon2id runs; the shipped five are the gate's own test.
    let host = host_with(SignLimits {
        min_interval: Duration::ZERO,
        wrong_password_limit: 1,
        lockout: Duration::from_secs(60),
    });
    host.create(secret("pw")).await.unwrap();
    let conn = Uuid::new_v4();
    let wrong = host
        .sign_challenge(conn, Some(ORIGIN.into()), request(), secret("not it"))
        .await;
    assert_eq!(reason(wrong), IdentityRefusal::WrongPassword);
    let after = host
        .sign_challenge(conn, Some(ORIGIN.into()), request(), secret("pw"))
        .await;
    assert_eq!(reason(after), IdentityRefusal::RateLimited);
}

#[tokio::test]
async fn a_second_request_inside_the_interval_is_rate_limited() {
    // The interval is measured on the host's clock, not on how long Argon2id took: the test
    // states the time between requests, so a loaded box cannot move it past the interval.
    let (host, clock) = host_and_clock(SHIPPED_SIGN_LIMITS);
    host.create(secret("pw")).await.unwrap();
    let conn = Uuid::new_v4();
    let sign = || host.sign_challenge(conn, Some(ORIGIN.into()), request(), secret("pw"));
    assert!(sign().await.is_ok());
    let inside = SHIPPED_SIGN_LIMITS.min_interval - Duration::from_millis(1);
    clock.advance(inside);
    assert_eq!(reason(sign().await), IdentityRefusal::RateLimited);
    // Past the interval on the host's clock, however little wall time has passed.
    clock.advance(SHIPPED_SIGN_LIMITS.min_interval);
    assert!(
        sign().await.is_ok(),
        "admitted once the interval has passed"
    );
}

#[tokio::test]
async fn no_browser_origin_and_bad_fields_are_refused_before_the_password() {
    let host = host();
    host.create(secret("pw")).await.unwrap();
    let conn = Uuid::new_v4();
    let native = host
        .sign_challenge(conn, None, request(), secret("pw"))
        .await;
    assert_eq!(reason(native), IdentityRefusal::NoBrowserOrigin);
    let mut short = request();
    short.nonce.pop();
    let mut empty = request();
    empty.aud.clear();
    let mut long = request();
    long.issuer = "a".repeat(256);
    for bad in [short, empty, long] {
        let got = host
            .sign_challenge(conn, Some(ORIGIN.into()), bad, secret("pw"))
            .await;
        assert_eq!(reason(got), IdentityRefusal::BadField);
    }
}

#[tokio::test]
async fn no_identity_is_refused() {
    let got = host()
        .sign_challenge(Uuid::new_v4(), Some(ORIGIN.into()), request(), secret("pw"))
        .await;
    assert_eq!(reason(got), IdentityRefusal::NoIdentity);
}
