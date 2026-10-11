//! Which peer a reported P2P pair is about, for the session it was reported to.
//!
//! A `PeerSignal::Disconnect` carries the pair as its sender crafted it. The local SDK's report
//! of its own connection is (local, peer); the server's notice that a peer's whole session
//! ended is (leaver, recipient) -- Citadel-Protocol `session_manager.rs`, "Alerting {peer_cid}
//! that {session_cid} disconnected". Read as (local, peer), that notice named the session that
//! had just ended, so the recipient's agent cleaned up nothing and kept a sink the SDK had
//! dropped. The SDK itself takes "the CID that isn't ours" (`peer_cmd_packet.rs`).

/// The CID in `pair` that is not `local`; `None` when the pair is not `local`'s.
pub(super) fn far_side(local: u64, (first, second): (u64, u64)) -> Option<u64> {
    match (first == local, second == local) {
        (true, false) => Some(second),
        (false, true) => Some(first),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::far_side;

    const LOCAL: u64 = 7;
    const PEER: u64 = 9;

    #[test]
    fn either_orientation_names_the_far_peer() {
        assert_eq!(far_side(LOCAL, (LOCAL, PEER)), Some(PEER), "own report");
        assert_eq!(
            far_side(LOCAL, (PEER, LOCAL)),
            Some(PEER),
            "the server's notice that the peer's session ended"
        );
    }

    #[test]
    fn a_pair_without_the_local_session_is_not_its_peer() {
        assert_eq!(far_side(LOCAL, (PEER, PEER + 1)), None);
        assert_eq!(far_side(LOCAL, (LOCAL, LOCAL)), None);
    }
}
