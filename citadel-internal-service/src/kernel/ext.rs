use crate::kernel::media::{MediaLaneRx, MediaLaneTx};
use crate::kernel::{send_to_kernel, sink_send_payload, Connection};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServicePayload, InternalServiceResponse, ServiceConnectionAccepted,
};
use citadel_sdk::logging::{debug, error, info, warn};
use citadel_sdk::prelude::Ratchet;
use futures::StreamExt;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use uuid::Uuid;

use citadel_internal_service_types::InternalServiceRequest;

pub trait IOInterfaceExt: IOInterface {
    #[allow(clippy::too_many_arguments)]
    fn spawn_connection_handler<R: Ratchet>(
        &mut self,
        mut sink: Self::Sink,
        mut stream: Self::Stream,
        to_kernel: UnboundedSender<(InternalServiceRequest, Uuid)>,
        mut from_kernel: UnboundedReceiver<InternalServiceResponse>,
        mut media_from_kernel: MediaLaneRx,
        conn_id: Uuid,
        tcp_connection_map: Arc<RwLock<HashMap<Uuid, UnboundedSender<InternalServiceResponse>>>>,
        media_lanes: Arc<RwLock<HashMap<Uuid, MediaLaneTx>>>,
        server_connection_map: Arc<RwLock<HashMap<u64, Connection<R>>>>,
        orphan_sessions: Arc<RwLock<HashMap<Uuid, bool>>>,
    ) {
        tokio::task::spawn(async move {
            let write_task = async {
                let response =
                    InternalServiceResponse::ServiceConnectionAccepted(ServiceConnectionAccepted {
                        cid: 0,
                        request_id: Some(conn_id),
                    });

                if let Err(err) = sink_send_payload::<Self>(response, &mut sink).await {
                    error!(target: "citadel", "Failed to send to client: {err:?}");
                    return;
                }

                // Media rides its own bounded lane, drained here alongside the
                // reliable one. Two queues, one socket: control cannot be
                // dropped and media cannot be allowed to accumulate, and a
                // single queue can only offer one of those.
                let mut media_open = true;
                loop {
                    let kernel_response = tokio::select! {
                        // Biased, and control first. A call saturating the
                        // socket must never delay a session close or a response
                        // the client is blocked waiting on -- under exactly the
                        // congestion this lane exists for, an unbiased select
                        // would hand roughly half the socket to stale video.
                        biased;
                        response = from_kernel.recv() => match response {
                            Some(response) => response,
                            // The reliable side ending is the connection
                            // ending; queued media has no one left to reach.
                            None => break,
                        },
                        frame = media_from_kernel.recv(), if media_open => match frame {
                            Some(frame) => frame,
                            None => {
                                // Disable the branch rather than break: control
                                // traffic outlives any one call.
                                media_open = false;
                                continue;
                            }
                        },
                    };
                    debug!(target: "citadel", "Sending kernel response to client: {:?}", kernel_response);
                    if let Err(err) = sink_send_payload::<Self>(kernel_response, &mut sink).await {
                        error!(target: "citadel", "Failed to send to client: {err:?}");
                        return;
                    }
                }
            };

            let read_task = async {
                while let Some(message) = stream.next().await {
                    match message {
                        Ok(message) => {
                            if let InternalServicePayload::Request(request) = message {
                                if let Err(err) = send_to_kernel(request, &to_kernel, conn_id) {
                                    error!(target: "citadel", "Failed to send to kernel: {:?}", err);
                                    break;
                                }
                            }
                        }
                        Err(_) => {
                            warn!(target: "citadel", "Bad message from client");
                        }
                    }
                }
                debug!(target: "citadel", "Disconnected connection {conn_id:?}");
            };

            tokio::select! {
                res0 = write_task => res0,
                res1 = read_task => res1,
            }

            tcp_connection_map.write().remove(&conn_id);
            retire_media_lane(&media_lanes, &conn_id);

            // ALWAYS preserve sessions when TCP drops.
            //
            // Sessions should persist across page navigations and reconnections.
            // This is the default behavior for modern web apps where users can:
            // - Navigate between pages
            // - Refresh the page
            // - Have multiple tabs open
            //
            // Sessions are only explicitly cleaned up via:
            // 1. Disconnect request (user-initiated logout)
            // 2. Deregister request (account deletion)
            // 3. GetSessions reconciliation (sync with SDK state)
            //
            // The orphan_sessions map is no longer used for cleanup decisions.
            // We preserve ALL sessions regardless of orphan mode setting.

            let (preserved_session_count, all_sessions, preserved_sessions_info) = {
                let lock = server_connection_map.read();
                let all: Vec<(u64, String)> = lock
                    .iter()
                    .map(|(cid, conn)| (*cid, conn.username.clone()))
                    .collect();
                let preserved: Vec<(u64, String)> = lock
                    .iter()
                    .filter(|(_, conn)| {
                        conn.associated_localhost_connection.load(Ordering::Relaxed) == conn_id
                    })
                    .map(|(cid, conn)| (*cid, conn.username.clone()))
                    .collect();
                (preserved.len(), all, preserved)
            };

            info!(target: "citadel", "[TCP_DISCONNECT] Connection {conn_id:?} closed. Preserving all sessions.");
            info!(target: "citadel", "[TCP_DISCONNECT] Total sessions in map: {:?}", all_sessions);
            info!(target: "citadel", "[TCP_DISCONNECT] Sessions associated with THIS connection ({conn_id:?}): {:?}", preserved_sessions_info);
            info!(target: "citadel", "[TCP_DISCONNECT] Preserved {} sessions for reconnection", preserved_session_count);

            // Clean up the orphan_sessions entry if it exists (no longer used for decisions)
            orphan_sessions.write().remove(&conn_id);
        });
    }
}

impl<T: IOInterface> IOInterfaceExt for T {}

/// Take a dropped connection's media lane out of the map and close it.
///
/// Closing as well as dropping is the point: a pump still holding a producer
/// handle learns the client is gone and stops, instead of decoding frames into
/// a queue nobody will ever read.
///
/// Its own function because the pump's own test performs this action itself --
/// the fixture calls `lane_tx.close()` before spawning -- so deleting the close
/// from the connection-drop path left that test green while a dropped WebSocket
/// leaked a pump per call, forever.
pub(crate) fn retire_media_lane(
    media_lanes: &Arc<RwLock<HashMap<Uuid, MediaLaneTx>>>,
    conn_id: &Uuid,
) {
    if let Some(lane) = media_lanes.write().remove(conn_id) {
        lane.close();
    }
}

#[cfg(test)]
mod lane_retirement_tests {
    use super::retire_media_lane;
    use crate::kernel::media::lane::{media_lane, PushOutcome};
    use crate::kernel::media::MediaLaneTx;
    use citadel_internal_service_types::InternalServiceResponse;
    use parking_lot::RwLock;
    use std::collections::HashMap;
    use std::sync::Arc;
    use uuid::Uuid;

    fn lanes() -> Arc<RwLock<HashMap<Uuid, MediaLaneTx>>> {
        Arc::new(RwLock::new(HashMap::new()))
    }

    fn a_frame() -> InternalServiceResponse {
        InternalServiceResponse::MediaGapNotification(
            citadel_internal_service_types::MediaGapNotification {
                cid: 1,
                peer_cid: 2,
                track: 0,
                missing_from: 1,
                missing_to: 2,
                request_id: None,
            },
        )
    }

    /// The pump's own test closes the lane itself before spawning, so it says
    /// nothing about whether the connection-drop path does. Deleting the close
    /// left that test green while every dropped WebSocket leaked a pump.
    #[test]
    fn retiring_a_lane_closes_it_so_a_live_pump_stops() {
        let map = lanes();
        let conn = Uuid::new_v4();
        let (tx, _rx) = media_lane(8);
        map.write().insert(conn, tx.clone());

        assert!(
            !matches!(tx.push(a_frame()), PushOutcome::Closed),
            "the lane must accept frames before the connection drops"
        );

        retire_media_lane(&map, &conn);

        assert!(
            matches!(tx.push(a_frame()), PushOutcome::Closed),
            "a pump still holding a producer handle must learn the client is gone"
        );
    }

    #[test]
    fn retiring_removes_the_lane_from_the_map() {
        let map = lanes();
        let conn = Uuid::new_v4();
        let (tx, _rx) = media_lane(8);
        map.write().insert(conn, tx);

        retire_media_lane(&map, &conn);

        assert!(
            map.read().is_empty(),
            "a dropped connection must not keep its lane"
        );
    }

    #[test]
    fn retiring_a_connection_with_no_lane_is_harmless() {
        // The common case: a connection that never opened a call.
        let map = lanes();
        retire_media_lane(&map, &Uuid::new_v4());
        assert!(map.read().is_empty());
    }

    #[test]
    fn retiring_one_connection_leaves_another_alone() {
        let map = lanes();
        let mine = Uuid::new_v4();
        let theirs = Uuid::new_v4();
        let (my_tx, _my_rx) = media_lane(8);
        let (their_tx, _their_rx) = media_lane(8);
        map.write().insert(mine, my_tx);
        map.write().insert(theirs, their_tx.clone());

        retire_media_lane(&map, &mine);

        assert!(
            !matches!(their_tx.push(a_frame()), PushOutcome::Closed),
            "one client's disconnect must not end another's call"
        );
    }
}
