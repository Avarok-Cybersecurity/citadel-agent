use crate::kernel::apps::kv::APP_STORAGE_PREFIX;
use crate::kernel::apps::settings::AGENT_KEY_PREFIX;
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::updates::AUTO_INSTALL_KEY;
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
mod agent_settings_tests;
#[cfg(test)]
mod app_storage_tests;

/// What a window is told when its LocalDB request names an app's record.
pub(crate) const APP_STORAGE_REFUSED: &str = "app: keys are the agent's apps storage";
/// What a window is told when its LocalDB request names one of the agent's own settings.
pub(crate) const AGENT_SETTINGS_REFUSED: &str = "agent: keys are the agent's own settings";

/// Why a window may not name `key`, if it is one only the agent reads and writes: an app's
/// `local` record (kernel/apps/kv, through `AppKv` and its namespace), or one of the agent's
/// settings (kernel/apps/settings.rs, and the updater's in kernel/updates.rs, which shipped
/// under its own name before `agent:` was reserved), through their owned requests.
pub(crate) fn reserved_key(key: &str) -> Option<&'static str> {
    if key.starts_with(APP_STORAGE_PREFIX) {
        Some(APP_STORAGE_REFUSED)
    } else if key.starts_with(AGENT_KEY_PREFIX) || key == AUTO_INSTALL_KEY {
        Some(AGENT_SETTINGS_REFUSED)
    } else {
        None
    }
}

/// Why a window's keyed LocalDB request is refused, if it names a reserved key. A window that
/// could would read, overwrite or delete another app's storage, or grant itself what only the
/// member may, past every check the owned requests make. The agent's own path
/// (requests/agent_own.rs) is not asked this.
pub(crate) fn names_reserved_key(command: &InternalServiceRequest) -> Option<&'static str> {
    match command {
        InternalServiceRequest::LocalDBGetKV { key, .. }
        | InternalServiceRequest::LocalDBSetKV { key, .. }
        | InternalServiceRequest::LocalDBDeleteKV { key, .. } => reserved_key(key),
        _ => None,
    }
}

/// A window's `LocalDBGetAllKV` answer, without the reserved records.
pub(crate) fn hide_reserved_keys(mut handled: HandledRequestResult) -> HandledRequestResult {
    if let InternalServiceResponse::LocalDBGetAllKVSuccess(success) = &mut handled.response {
        success.map.retain(|key, _| reserved_key(key).is_none());
    }
    handled
}

/// The keys a window's `LocalDBClearAllKV` removes: every one but the reserved records.
pub(crate) fn keys_a_window_clears(stored: &HashMap<String, Vec<u8>>) -> Vec<String> {
    stored
        .keys()
        .filter(|key| reserved_key(key).is_none())
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
