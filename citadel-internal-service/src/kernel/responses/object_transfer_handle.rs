use crate::kernel::{spawn_tick_updater, CitadelWorkspaceService};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{FileTransferRequestNotification, InternalServiceResponse};
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::{
    NetworkError, ObjectTransferHandle, ObjectTransferOrientation, Ratchet, TransferType,
};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

/// Sends `response` to the localhost client `tcp_uuid` and reports whether it
/// was actually handed to a live client.
///
/// This exists alongside `send_response_to_tcp_client` because that helper
/// deliberately treats a missing uuid as Ok(()) (a dropped response must not
/// crash the service). For a transfer OFFER that answer is not enough: the
/// caller inserted a handler-map entry that only the notified client can ever
/// remove, so "the client is gone" must be distinguishable from "delivered" —
/// otherwise the entry is pinned for the life of the session.
pub(crate) fn deliver_offer_to_localhost_client(
    clients: &Arc<RwLock<HashMap<Uuid, UnboundedSender<InternalServiceResponse>>>>,
    response: InternalServiceResponse,
    tcp_uuid: Uuid,
) -> bool {
    match clients.read().get(&tcp_uuid) {
        Some(sender) => sender.send(response).is_ok(),
        None => false,
    }
}

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    object_transfer_handle: ObjectTransferHandle,
) -> Result<(), NetworkError> {
    let metadata = object_transfer_handle.handle.metadata.clone();
    let object_id = metadata.object_id;
    let implicated_cid = object_transfer_handle.session_cid;
    let peer_cid = if object_transfer_handle.handle.receiver != implicated_cid {
        object_transfer_handle.handle.receiver
    } else {
        object_transfer_handle.handle.source
    };
    let object_transfer_handler = object_transfer_handle.handle;

    citadel_sdk::logging::info!(target: "citadel", "Orientation: {:?}", object_transfer_handler.orientation);
    citadel_sdk::logging::info!(target: "citadel", "ObjectTransferHandle has implicated_cid: {implicated_cid:?} and peer_cid {peer_cid:?}");

    // When we receive a handle, there are two possibilities:
    // A: We are the sender of the file transfer, in which case we can assume the adjacent node
    // already accepted the file transfer request, and therefore we can spawn a task to forward
    // the ticks immediately
    //
    // B: We are the receiver of the file transfer. We need to wait for the TCP client to accept
    // the request, thus, we need to store it. UNLESS, this is an revfs pull, in which case we
    // allow the transfer to proceed immediately since the protocol auto accepts these requests
    if let ObjectTransferOrientation::Receiver { is_revfs_pull } =
        object_transfer_handler.orientation
    {
        info!(target: "citadel", "Receiver Obtained ObjectTransferHandler");

        let mut server_connection_map = this.server_connection_map.write();
        if let Some(connection) = server_connection_map.get_mut(&implicated_cid) {
            // Resolve the session's CURRENT TCP connection live from the map
            // (NOT a stale/captured handler value). This is read under the
            // write lock right before delivery and reflects any ClaimSession
            // that re-pointed the session — the same pattern peer_channel_
            // created.rs uses for its one-shot PeerConnectSuccess delivery.
            let current_tcp_uuid = connection
                .associated_localhost_connection
                .load(Ordering::Relaxed);

            if is_revfs_pull {
                // Reclaim the browser's DownloadFile request_id (registered in
                // requests/file/download.rs — PullObject cannot carry it). The
                // browser correlates ticks by that id; with `None` here the
                // ticks fell back to the TCP-connection uuid, matched nothing,
                // and every completed download was reported as a 30s-timeout
                // failure. `peer_cid` is the same scope key download.rs
                // registered under: the peer's cid for P2P, 0 for c2s
                // (handle.source == C2S_IDENTITY_CID).
                let request_id = connection.revfs_correlations.take_pull(peer_cid);
                spawn_tick_updater(
                    object_transfer_handler,
                    implicated_cid,
                    Some(peer_cid),
                    &mut server_connection_map,
                    this.tx_to_localhost_clients.clone(),
                    request_id,
                );
            } else if matches!(
                metadata.transfer_type,
                TransferType::RemoteEncryptedVirtualFilesystem { .. }
            ) {
                // A REVFS *push* from a peer: accept it here, without asking
                // the browser. REVFS storage writes are an internal protocol
                // mechanism — the uploader's page has already recorded the
                // file and synced the tree op — not a user-facing transfer
                // offer. Nothing in the UI answers the accept prompt for
                // them, so pending this like a standard transfer meant no
                // bytes were EVER streamed: both trees listed a downloadable
                // file that existed nowhere, and the uploader's Sender
                // handle (whose TransferComplete the browser now awaits)
                // never arrived, because the receiver only acks the file
                // header after acceptance. The server kernel auto-accepts
                // for exactly the same reason; this mirrors it for the
                // peer-hosted scope.
                let mut handler = object_transfer_handler;
                match handler.accept() {
                    Ok(()) => {
                        info!(target: "citadel", "Auto-accepted inbound REVFS push from peer {peer_cid} for cid {implicated_cid}");
                        // Drain the status stream so reception completes; the
                        // receiving browser issued no request, so there is no
                        // request_id to stamp these ticks with.
                        spawn_tick_updater(
                            handler,
                            implicated_cid,
                            Some(peer_cid),
                            &mut server_connection_map,
                            this.tx_to_localhost_clients.clone(),
                            None,
                        );
                    }
                    Err(err) => {
                        warn!(target: "citadel", "[ObjectTransferHandle] Failed to auto-accept REVFS push from peer {peer_cid} for cid {implicated_cid}: {err:?}");
                    }
                }
            } else {
                // Send an update to the TCP client that way they can choose to accept or reject the transfer
                let response = InternalServiceResponse::FileTransferRequestNotification(
                    FileTransferRequestNotification {
                        cid: implicated_cid,
                        peer_cid,
                        metadata,
                        request_id: None,
                    },
                );

                // This insert is what makes the offer answerable at all — and
                // the only paths that remove it are: (1) the user answering it
                // (requests/file/respond_file_transfer.rs takes the handle),
                // (2) the reclaim-and-decline below when the offer
                // notification cannot be delivered, and (3) session teardown
                // (disconnect/deregister), which drops the Connection and its
                // handler maps with it. Still NOT removed: an offer that WAS
                // delivered to a live client that then never answers — that
                // is an open offer by design, and it lives until (1) or (3).
                connection.add_object_transfer_handler(
                    peer_cid,
                    object_id,
                    Some(object_transfer_handler),
                );

                drop(server_connection_map);

                // Deliver to the TCP connection currently associated with this
                // session. A previous version broadcast to every live TCP
                // entry as a workaround for stale-UUID delivery during
                // ClaimSession races, but that leaked file metadata to any
                // other session multiplexed through the same internal
                // service (one IS process can host sessions for multiple
                // distinct users). The single-TCP-per-browser architecture
                // invariant means `associated_localhost_connection` is the
                // sole authoritative target.
                //
                // Delivery has to be CHECKED here, not fire-and-forget:
                // `send_response_to_tcp_client` maps "uuid not in the live
                // map" to Ok(()) so ordinary response paths don't crash the
                // service, but for a pending transfer offer that contract hid
                // the abandon path — a notification dropped on the floor left
                // an entry the user could never answer (they never saw the
                // offer), pinned for the life of a session that deliberately
                // survives TCP drops. When delivery fails, reclaim the entry
                // and decline the transfer so the remote sender gets a
                // rejection instead of waiting forever.
                if !deliver_offer_to_localhost_client(
                    &this.tx_to_localhost_clients,
                    response,
                    current_tcp_uuid,
                ) {
                    warn!(target: "citadel", "[ObjectTransferHandle] FileTransferRequestNotification for cid={implicated_cid}, peer_cid={peer_cid}, object_id={object_id:?} was undeliverable (localhost connection {current_tcp_uuid:?} gone) - reclaiming and declining the pending offer");
                    let reclaimed = this
                        .server_connection_map
                        .write()
                        .get_mut(&implicated_cid)
                        .and_then(|conn| conn.take_file_transfer_handle(peer_cid, object_id))
                        .flatten();
                    if let Some(mut handler) = reclaimed {
                        if let Err(err) = handler.decline() {
                            warn!(target: "citadel", "[ObjectTransferHandle] Failed to decline undeliverable transfer offer from peer {peer_cid} for cid {implicated_cid}: {err:?}");
                        }
                    }
                }
            }
        }
    } else {
        // Sender - Must spawn a task to relay status updates to TCP client. When receiving this handle,
        // we know the opposite node agreed to the connection thus we can spawn
        let mut server_connection_map = this.server_connection_map.write();
        info!(target: "citadel", "Sender Obtained ObjectTransferHandler");
        // A REVFS push's Sender ticks are the uploader's ONLY completion
        // signal (SendFileRequestSuccess just means "queued"), so reclaim the
        // browser's SendFile request_id registered in requests/file/upload.rs.
        // The scope key mirrors upload.rs: for a c2s push the handle carries
        // source == receiver == session_cid, so the computed `peer_cid` here
        // IS `implicated_cid` — which is what upload.rs registered under
        // (`peer_cid.unwrap_or(cid)`). Standard file transfers register
        // nothing, so they keep the legacy TCP-uuid fallback.
        let request_id = if matches!(
            object_transfer_handler.metadata.transfer_type,
            TransferType::RemoteEncryptedVirtualFilesystem { .. }
        ) {
            server_connection_map
                .get_mut(&implicated_cid)
                .and_then(|conn| conn.revfs_correlations.take_push(peer_cid))
        } else {
            None
        };
        spawn_tick_updater(
            object_transfer_handler,
            implicated_cid,
            Some(peer_cid),
            &mut server_connection_map,
            this.tx_to_localhost_clients.clone(),
            request_id,
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::deliver_offer_to_localhost_client;
    use citadel_internal_service_types::{GroupMessageFailure, InternalServiceResponse};
    use parking_lot::RwLock;
    use std::collections::HashMap;
    use std::sync::Arc;
    use uuid::Uuid;

    // Any variant works here; the helper never inspects the payload. This is
    // simply the cheapest one to construct.
    fn arbitrary_response() -> InternalServiceResponse {
        InternalServiceResponse::GroupMessageFailure(GroupMessageFailure {
            cid: 0,
            message: "test payload".to_string(),
            request_id: None,
        })
    }

    #[test]
    fn absent_client_is_reported_undeliverable() {
        // The M4 defect: a uuid missing from the live map was treated as a
        // successful delivery (Ok(())), so the pending-offer entry inserted
        // just before was never reclaimed. The helper must report failure.
        let clients = Arc::new(RwLock::new(HashMap::new()));
        assert!(
            !deliver_offer_to_localhost_client(&clients, arbitrary_response(), Uuid::new_v4()),
            "a transfer offer aimed at a localhost connection that no longer exists must \
             be reported undeliverable so the caller reclaims the pending-offer entry"
        );
    }

    #[test]
    fn closed_client_channel_is_reported_undeliverable() {
        let uuid = Uuid::new_v4();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        drop(rx);
        let clients = Arc::new(RwLock::new(HashMap::from([(uuid, tx)])));
        assert!(
            !deliver_offer_to_localhost_client(&clients, arbitrary_response(), uuid),
            "a send into a closed client channel must be reported undeliverable"
        );
    }

    #[test]
    fn live_client_receives_the_offer() {
        let uuid = Uuid::new_v4();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let clients = Arc::new(RwLock::new(HashMap::from([(uuid, tx)])));
        assert!(deliver_offer_to_localhost_client(
            &clients,
            arbitrary_response(),
            uuid
        ));
        assert!(
            rx.try_recv().is_ok(),
            "the live client must get the payload"
        );
    }
}
