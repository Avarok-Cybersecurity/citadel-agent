//! A server that still holds the session that dropped is waited out, not given up on.
//! The live case: a reset only this side saw, every attempt refused as "already
//! connected" for ten minutes, then the session removed and the account's chip gone.

use super::policy::*;
use citadel_io::ErrorCode;
use std::time::Duration;

const P: ReconnectPolicy = SERVER_RECONNECT;

/// The SDK server's refusal, as the reconnect receives it (ErrorCode::Generic).
const REFUSAL: &str = "Session Already Connected, or, is in the process of disconnection and an earlier connection attempt beat this connection. Not allowing this connection";

const HOUR: Duration = Duration::from_secs(3600);

#[test]
fn the_server_refusing_a_second_session_is_its_own_kind() {
    for code in [ErrorCode::Generic, ErrorCode::RemoteConnectFailed] {
        assert_eq!(classify(code, REFUSAL), FailureKind::ServerHoldsSession);
    }
    assert_eq!(
        classify(ErrorCode::Generic, &format!("Preconnect failed: {REFUSAL}")),
        FailureKind::ServerHoldsSession,
        "a server's own prefix does not hide it"
    );
    assert_eq!(
        classify(
            ErrorCode::Generic,
            "retrying since Session Already Connected"
        ),
        FailureKind::Transient,
        "merely mentioned mid-sentence, it is not the refusal"
    );
}

#[test]
fn it_is_waited_out_for_as_long_as_the_server_can_hold_the_session() {
    // The SDK's defaults: a 45-minute keep-alive timeout, checked every 15 minutes.
    assert_eq!(P.server_holds_session_for, HOUR);
    assert_eq!(P.limit(FailureKind::ServerHoldsSession), HOUR);
    assert_eq!(P.limit(FailureKind::Transient), P.give_up_after);

    let past_ten_minutes = P.give_up_after + Duration::from_secs(1);
    assert_eq!(
        P.after_failure(25, past_ten_minutes, FailureKind::ServerHoldsSession),
        Next::RetryAfter(P.max_delay),
        "ten minutes in, the server may still hold it for fifty more"
    );
    assert_eq!(
        P.after_failure(25, past_ten_minutes, FailureKind::Transient),
        Next::GiveUp(GiveUp::OutOfTime),
        "any other failure keeps the ten-minute limit"
    );
    assert_eq!(
        P.after_failure(200, HOUR, FailureKind::ServerHoldsSession),
        Next::GiveUp(GiveUp::OutOfTime),
        "past the server's own expiry, still refusing is not a dead session: it is reported"
    );
}

#[test]
fn a_session_with_its_own_keep_alive_is_waited_out_by_it() {
    let short = P.for_keep_alive(Some(Duration::from_secs(60)));
    assert_eq!(
        short.server_holds_session_for,
        Duration::from_secs(60 + 15 * 60)
    );
    assert_eq!(
        P.for_keep_alive(None),
        P,
        "no keep-alive named: the SDK's default applies"
    );
    let off = P.for_keep_alive(Some(Duration::ZERO));
    assert_eq!(
        off.limit(FailureKind::ServerHoldsSession),
        P.give_up_after,
        "keep-alives off: the server never expires it, so there is nothing to wait for"
    );
}
