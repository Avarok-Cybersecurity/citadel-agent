//! The shapes of the LocalDB keys the agent's own stores use, in one place.

use crate::kernel::conversations::store::PAGINATED_PREFIX;

/// The account a scoped conversation record belongs to:
/// `msgs_with_peer_{owner}_with_{peer}_…`. A legacy, peer-only key names no owner.
pub(crate) fn conversation_owner(key: &str) -> Option<u64> {
    let rest = key.strip_prefix(PAGINATED_PREFIX)?;
    let (owner, _) = rest.split_once("_with_")?;
    owner.parse().ok()
}

/// The pause record the web UI keeps for one contact, in the session's own
/// LocalDB (`p2p-pause/pause-rules.ts`, `pauseKey`).
pub(crate) fn pause_key(peer_cid: u64) -> String {
    format!("p2p_paused_peer_{peer_cid}")
}
