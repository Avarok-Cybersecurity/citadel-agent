use citadel_sdk::prelude::{NodeRemote, ProtocolRemoteExt, Ratchet, SymmetricIdentifierHandleRef};

pub mod clear_all_kv;
pub mod delete_kv;
pub mod get_all_kv;
pub mod get_kv;
pub mod set_kv;

/// This function misuses `propose_target` to reduce the need of a new remote type or using dyanmic
/// dispatch. This function should only be used for local backend KV requests, and strictly uses CIDs
/// to not trigger backend searches
pub(crate) async fn generate_remote<R: Ratchet>(
    node_remote: &NodeRemote<R>,
    cid: u64,
    peer_cid: Option<u64>,
) -> SymmetricIdentifierHandleRef<'_, R> {
    node_remote
        .propose_target(cid, peer_cid.unwrap_or(0))
        .await
        .expect("Should not fail to find target")
}
