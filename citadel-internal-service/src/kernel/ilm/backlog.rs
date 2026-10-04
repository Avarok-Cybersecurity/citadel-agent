//! How many messages the account's ILM still owes each peer. Read from the store ILM itself
//! reads its outbound queue from, so it is what ILM will retransmit, not a second count.

use super::channel::AgentChannel;
use super::HostIo;
use citadel_internal_service_connector::messenger::backend::CitadelWorkspaceBackend;
use intersession_layer_messaging::{Backend, MessageMetadata};
use std::collections::HashMap;
use std::sync::Arc;

pub(crate) async fn pending_by_peer(
    cid: u64,
    io: Arc<dyn HostIo>,
) -> Result<HashMap<u64, u32>, String> {
    let backend = CitadelWorkspaceBackend::with_channel(cid, AgentChannel::new(io));
    let pending = backend
        .get_pending_outbound()
        .await
        .map_err(|err| format!("{err:?}"))?;
    let mut by_peer: HashMap<u64, u32> = HashMap::new();
    for message in pending {
        let count = by_peer.entry(message.destination_id()).or_default();
        *count = count.saturating_add(1);
    }
    Ok(by_peer)
}
