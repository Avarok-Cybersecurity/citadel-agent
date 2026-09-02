use citadel_sdk::prelude::{
    NetworkError, NodeRemote, ProtocolRemoteExt, Ratchet, SymmetricIdentifierHandleRef,
};

pub mod clear_all_kv;
pub mod delete_kv;
pub mod get_all_kv;
pub mod get_kv;
pub mod set_kv;

/// This function misuses `propose_target` to reduce the need of a new remote type or using dyanmic
/// dispatch. This function should only be used for local backend KV requests, and strictly uses CIDs
/// to not trigger backend searches
/// Returns `Err` when the CID names no locally-known account.
///
/// This was `.expect("Should not fail to find target")`, and the expectation is
/// wrong: `propose_target` calls `get_session_cid`, which does
/// `.ok_or(RemoteUserDoesNotExist)?` on an `Option` — an unknown CID yields
/// `Err`. The `.expect` then panicked inside the per-request spawned task, so
/// **no response was ever sent** and the browser waited out its own timeout with
/// nothing to show.
///
/// That is not a theoretical input. The dev stack's in-memory backend drops all
/// accounts on every internal-service restart while browsers keep their stored
/// CIDs, so the first stale LocalDB read after a restart hits it — and
/// `LocalDBGetKV` is exempt from the ownership gate, so it arrives unfiltered.
pub(crate) async fn generate_remote<R: Ratchet>(
    node_remote: &NodeRemote<R>,
    cid: u64,
    peer_cid: Option<u64>,
) -> Result<SymmetricIdentifierHandleRef<'_, R>, NetworkError> {
    node_remote.propose_target(cid, peer_cid.unwrap_or(0)).await
}
