//! A peer notification goes to one session, or to nobody.
//!
//! `peer_event.rs` had a fallback: when the uuid recorded on the event was not
//! in the localhost-client map, it sent the response to EVERY active localhost
//! connection, with the comment "The clients will filter based on CID to only
//! process messages meant for their sessions".
//!
//! Every browser on this internal service — every account signed in on this
//! machine — therefore received the peer-register, peer-connect and disconnect
//! notifications of every other, and was trusted to discard them. Who is talking
//! to whom is precisely the thing a peer notification discloses, and client-side
//! filtering is not a boundary.
//!
//! The stale uuid is real: a reload, a tab close or a reconnect mints a new
//! localhost connection while the session and its CID persist. But the session
//! records its current one in `associated_localhost_connection`, an `AtomicUuid`
//! updated on reconnect — so the answer is to re-resolve through the CID, and to
//! drop with a warning when even that finds nothing. That is what
//! `send_response_to_tcp_client` in kernel/mod.rs already does; this file was the
//! only remaining exception.
//!
//! Asserted against the source because the behaviour lives at the service's
//! fan-out boundary: reproducing it needs two authenticated accounts on one
//! running internal service, which the unit suite has no way to stand up. The
//! limit is real and stated: this pins that no broadcast loop exists, not that
//! every delivery reaches the right tab.

const PEER_EVENT: &str = include_str!("../src/kernel/responses/peer_event.rs");

/// Comments stripped, so prose describing the old behaviour cannot satisfy — or
/// trip — a check about the code.
fn code() -> String {
    PEER_EVENT
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn no_peer_notification_is_sent_to_every_localhost_connection() {
    let code = code();

    assert!(
        !code.contains("for (uuid, sender) in tcp_map.iter()"),
        "peer_event.rs broadcasts to every localhost connection again: every \
         account signed in on this machine receives every other account's peer \
         notifications and is trusted to filter them client-side"
    );
    // The shape rather than the exact line, so a rename does not smuggle it back.
    assert!(
        !code.contains("tcp_map.iter()"),
        "peer_event.rs iterates the whole localhost-client map; the only correct \
         reason to do that is a broadcast, which is what this forbids"
    );
}

#[test]
fn the_recorded_uuid_is_re_resolved_through_the_session() {
    let code = code();

    assert!(
        code.contains("fn send_response_for_session"),
        "the per-session sender is gone; if it was renamed, update this test"
    );
    assert!(
        code.contains("server_connection_map"),
        "send_response_for_session no longer consults the connection map, so a \
         stale uuid is no longer re-resolved and reconnected tabs lose their \
         peer notifications"
    );
    assert!(
        code.contains("associated_localhost_connection"),
        "the session's live localhost connection is no longer read"
    );

    // Every call site must go through it. Three: PeerSignal::Disconnect,
    // PostRegister and PostConnect. The definition carries generics
    // (`send_response_for_session<T: IOInterface, R: Ratchet>`) so it does not
    // match this pattern — which is what makes the count exactly the call sites.
    let calls = code.matches("send_response_for_session(").count();
    assert_eq!(
        calls, 3,
        "expected three call sites, found {calls} — either a peer notification \
         is being delivered by some other route, or a new one was added without \
         going through the per-session sender"
    );
}
