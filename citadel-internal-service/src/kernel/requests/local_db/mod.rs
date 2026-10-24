use crate::kernel::apps::kv::APP_STORAGE_PREFIX;
use crate::kernel::requests::HandledRequestResult;
use citadel_internal_service_types::{InternalServiceRequest, InternalServiceResponse};
use citadel_sdk::prelude::{
    NetworkError, NodeRemote, ProtocolRemoteExt, Ratchet, SymmetricIdentifierHandleRef,
};
use std::collections::HashMap;

pub mod clear_all_kv;
pub mod delete_kv;
pub mod get_all_kv;
pub mod get_kv;
pub mod set_kv;

#[cfg(test)]
mod app_storage_tests;

/// What a window is told when its LocalDB request names an app's record.
pub(crate) const APP_STORAGE_REFUSED: &str = "app: keys are the agent's apps storage";

/// Whether `key` is one of the apps' `local` records (kernel/apps/kv), which only the agent
/// reads and writes, through `AppKv` and its namespace.
pub(crate) fn is_app_storage_key(key: &str) -> bool {
    key.starts_with(APP_STORAGE_PREFIX)
}

/// Whether a window's keyed LocalDB request names an app's record. A window that could
/// would read, overwrite or delete another app's storage, or its index, past every check
/// `AppKv` makes. The agent's own path (requests/agent_own.rs) is not asked this.
pub(crate) fn names_app_storage(command: &InternalServiceRequest) -> bool {
    match command {
        InternalServiceRequest::LocalDBGetKV { key, .. }
        | InternalServiceRequest::LocalDBSetKV { key, .. }
        | InternalServiceRequest::LocalDBDeleteKV { key, .. } => is_app_storage_key(key),
        _ => false,
    }
}

/// A window's `LocalDBGetAllKV` answer, without the apps' records.
pub(crate) fn hide_app_storage(mut handled: HandledRequestResult) -> HandledRequestResult {
    if let InternalServiceResponse::LocalDBGetAllKVSuccess(success) = &mut handled.response {
        success.map.retain(|key, _| !is_app_storage_key(key));
    }
    handled
}

/// The keys a window's `LocalDBClearAllKV` removes: every one but the apps' records.
pub(crate) fn keys_a_window_clears(stored: &HashMap<String, Vec<u8>>) -> Vec<String> {
    stored
        .keys()
        .filter(|key| !is_app_storage_key(key))
        .cloned()
        .collect()
}

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
