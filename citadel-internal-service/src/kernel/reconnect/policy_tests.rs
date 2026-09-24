use super::policy::*;
use super::LinkState;
use citadel_io::ErrorCode;
use std::time::Duration;

const P: ReconnectPolicy = SERVER_RECONNECT;

#[test]
fn the_waits_double_from_half_a_second_and_stop_at_thirty() {
    let waits: Vec<u64> = (0..9)
        .map(|attempt| P.delay_before(attempt).as_millis() as u64)
        .collect();
    assert_eq!(
        waits,
        [500, 1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000, 30_000]
    );
}

#[test]
fn a_huge_attempt_number_is_capped_not_overflowed() {
    assert_eq!(P.delay_before(31), P.max_delay);
    assert_eq!(P.delay_before(32), P.max_delay);
    assert_eq!(P.delay_before(u32::MAX), P.max_delay);
}

#[test]
fn a_transient_failure_waits_the_next_step() {
    assert_eq!(
        P.after_failure(0, Duration::from_millis(600), FailureKind::Transient),
        Next::RetryAfter(Duration::from_secs(1))
    );
    assert_eq!(
        P.after_failure(7, Duration::from_secs(120), FailureKind::Transient),
        Next::RetryAfter(Duration::from_secs(30))
    );
}

#[test]
fn a_refusal_stops_at_once_however_early() {
    assert_eq!(
        P.after_failure(0, Duration::ZERO, FailureKind::Refused),
        Next::GiveUp(GiveUp::Refused)
    );
}

#[test]
fn it_gives_up_rather_than_start_an_attempt_past_ten_minutes() {
    assert_eq!(
        P.after_failure(20, Duration::from_secs(570), FailureKind::Transient),
        Next::RetryAfter(Duration::from_secs(30))
    );
    assert_eq!(
        P.after_failure(20, Duration::from_secs(571), FailureKind::Transient),
        Next::GiveUp(GiveUp::OutOfTime)
    );
}

#[test]
fn the_whole_schedule_ends_near_ten_minutes() {
    let mut elapsed = P.delay_before(0);
    let mut attempt = 0;
    loop {
        match P.after_failure(attempt, elapsed, FailureKind::Transient) {
            Next::RetryAfter(delay) => {
                elapsed += delay;
                attempt += 1;
            }
            Next::GiveUp(reason) => {
                assert_eq!(reason, GiveUp::OutOfTime);
                break;
            }
        }
    }
    assert!(elapsed <= P.give_up_after, "{elapsed:?}");
    assert!(elapsed > P.give_up_after - P.max_delay, "{elapsed:?}");
    assert!(attempt < 40, "{attempt} attempts");
}

#[test]
fn account_errors_raised_here_are_refusals() {
    for code in [
        ErrorCode::AccountClientNonExists,
        ErrorCode::AccountServerNonExists,
        ErrorCode::AccountInvalidUsername,
        ErrorCode::AccountInvalidPassword,
        ErrorCode::AccountDisengaged,
    ] {
        assert_eq!(classify(code, ""), FailureKind::Refused, "{code:?}");
    }
}

#[test]
fn a_server_refusal_is_recognised_by_its_rendered_text() {
    let remote = ErrorCode::RemoteConnectFailed;
    assert_eq!(classify(remote, "Invalid password"), FailureKind::Refused);
    assert_eq!(classify(remote, "Invalid username"), FailureKind::Refused);
    assert_eq!(
        classify(remote, "Client account does not exist: 42"),
        FailureKind::Refused
    );
    assert_eq!(
        classify(remote, "Account disengaged: 42"),
        FailureKind::Refused
    );
}

#[test]
fn an_unreachable_or_busy_server_is_transient() {
    let remote = ErrorCode::RemoteConnectFailed;
    assert_eq!(
        classify(remote, "Session Already Connected"),
        FailureKind::Transient
    );
    assert_eq!(
        classify(ErrorCode::RemoteKernelStreamDied, "connect"),
        FailureKind::Transient
    );
    assert_eq!(
        classify(ErrorCode::Generic, "Connection refused"),
        FailureKind::Transient
    );
    // The refusal's text must lead: a transport error that merely mentions one is not one.
    assert_eq!(
        classify(remote, "timed out after Invalid password"),
        FailureKind::Transient
    );
}

#[test]
fn only_an_up_session_is_reconnected() {
    assert_eq!(on_unrequested_drop(LinkState::Up), DropAction::Reconnect);
    assert_eq!(
        on_unrequested_drop(LinkState::Reconnecting),
        DropAction::AlreadyReconnecting
    );
    assert_eq!(on_unrequested_drop(LinkState::Ending), DropAction::Remove);
}
