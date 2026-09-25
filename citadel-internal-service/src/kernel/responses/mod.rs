use crate::kernel::{session_wait, CitadelWorkspaceService};

use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_sdk::logging::info;
use citadel_sdk::prelude::{NetworkError, NodeResult, Ratchet};

mod disconnect;
mod object_transfer_handle;
mod peer_channel_created;

mod group_channel_created;
pub(crate) mod group_event;
mod peer_event;

pub async fn handle_node_result<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    result: NodeResult<R>,
) -> Result<(), NetworkError> {
    info!(target: "citadel", "NODE EVENT RECEIVED WITH MESSAGE: {result:?}");
    match result {
        NodeResult::Disconnect(dc) => return disconnect::handle(this, dc).await,
        NodeResult::ObjectTransferHandle(object_transfer_handle) => {
            return object_transfer_handle::handle(this, object_transfer_handle).await
        }
        NodeResult::GroupChannelCreated(group_channel_created) => {
            let cid = group_channel_created.channel.cid();
            return session_wait::once_session_mapped(
                this,
                cid,
                "GroupChannelCreated",
                group_channel_created,
                |this, created| async move { group_channel_created::handle(&this, created).await },
                |created| group_channel_created::park(created.channel),
            )
            .await;
        }
        NodeResult::PeerChannelCreated(peer_channel_created) => {
            return peer_channel_created::handle(this, peer_channel_created).await
        }
        NodeResult::PeerEvent(event) => return peer_event::handle(this, event).await,

        NodeResult::GroupEvent(group_event) => {
            let cid = group_event.session_cid;
            return session_wait::once_session_mapped(
                this,
                cid,
                "GroupEvent",
                group_event,
                |this, event| async move { group_event::handle(&this, event).await },
                drop,
            )
            .await;
        }

        evt => {
            citadel_sdk::logging::warn!(target: "citadel", "Unhandled node result: {evt:?}")
        }
    }

    Ok(())
}
