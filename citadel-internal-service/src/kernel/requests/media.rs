//! Media session request handlers: open, send, close.
//!
//! The transport itself lives in `kernel::media`; this is the request plumbing
//! that owns the session's lifetime inside a peer connection.

use crate::kernel::media::MediaSession;
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, MediaSessionClosed, MediaSessionFailed,
    MediaSessionOpened,
};
use citadel_sdk::logging::info;
use citadel_sdk::prelude::Ratchet;
use uuid::Uuid;

fn failed(cid: u64, peer_cid: u64, request_id: Uuid, message: String) -> InternalServiceResponse {
    InternalServiceResponse::MediaSessionFailed(MediaSessionFailed {
        cid,
        peer_cid,
        message,
        request_id: Some(request_id),
    })
}

pub async fn handle_open<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::MediaOpen {
        request_id,
        cid,
        peer_cid,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };

    // Take the UDP receiver out from under the lock and release it before
    // awaiting. Holding the map's write guard across the await would stall every
    // other session while this one waits up to five seconds for a UDP channel.
    let udp_rx = {
        let mut map = this.server_connection_map.write();
        match map.get_mut(&cid).and_then(|conn| conn.peers.get_mut(&peer_cid)) {
            Some(peer) => {
                if peer.media.is_some() {
                    // Idempotent rather than an error: both sides of a call can
                    // race to open, and re-opening would drop the live pump and
                    // silence the call that is already working.
                    info!(target: "citadel", "[Media] session already open cid={cid} peer_cid={peer_cid}");
                    return Some(HandledRequestResult {
                        response: InternalServiceResponse::MediaSessionOpened(MediaSessionOpened {
                            cid,
                            peer_cid,
                            unreliable: true,
                            max_frame_bytes: MediaSession::max_frame_bytes(),
                            request_id: Some(request_id),
                        }),
                        uuid,
                    });
                }
                peer.udp_rx.take()
            }
            None => {
                return Some(HandledRequestResult {
                    response: failed(
                        cid,
                        peer_cid,
                        request_id,
                        format!("no peer connection to {peer_cid}; connect before starting a call"),
                    ),
                    uuid,
                })
            }
        }
    };

    let Some(udp_rx) = udp_rx else {
        return Some(HandledRequestResult {
            response: failed(
                cid,
                peer_cid,
                request_id,
                "this peer connection has no UDP channel, so it was established with UdpMode \
                 disabled; reconnect to the peer with UDP enabled to place a call"
                    .to_string(),
            ),
            uuid,
        });
    };

    let to_client = this.tx_to_localhost_clients.read().get(&uuid).cloned();
    let Some(to_client) = to_client else {
        return Some(HandledRequestResult {
            response: failed(cid, peer_cid, request_id, "client is gone".to_string()),
            uuid,
        });
    };

    let response = match MediaSession::open(udp_rx, cid, peer_cid, to_client).await {
        Ok(session) => {
            let mut map = this.server_connection_map.write();
            match map.get_mut(&cid).and_then(|conn| conn.peers.get_mut(&peer_cid)) {
                Some(peer) => {
                    peer.media = Some(Box::new(session));
                    InternalServiceResponse::MediaSessionOpened(MediaSessionOpened {
                        cid,
                        peer_cid,
                        unreliable: true,
                        max_frame_bytes: MediaSession::max_frame_bytes(),
                        request_id: Some(request_id),
                    })
                }
                // The peer disconnected while we waited for the UDP channel.
                // Dropping `session` here stops its pump, which is why it is not
                // leaked by this path.
                None => failed(
                    cid,
                    peer_cid,
                    request_id,
                    "peer disconnected while the media session was opening".to_string(),
                ),
            }
        }
        Err(e) => failed(cid, peer_cid, request_id, e.to_string()),
    };

    Some(HandledRequestResult { response, uuid })
}

pub async fn handle_send<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::MediaSend {
        request_id,
        cid,
        peer_cid,
        track,
        kind,
        timestamp,
        flags,
        payload,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };

    // Deliberately synchronous and lock-scoped: packetizing writes to per-track
    // sequence counters, and the send is a non-blocking queue push. There is no
    // await here, so the guard cannot be held across one.
    let result = {
        let mut map = this.server_connection_map.write();
        match map
            .get_mut(&cid)
            .and_then(|conn| conn.peers.get_mut(&peer_cid))
            .and_then(|peer| peer.media.as_mut())
        {
            Some(session) => session.send_frame(track, kind, timestamp, flags, payload),
            None => {
                return Some(HandledRequestResult {
                    response: failed(
                        cid,
                        peer_cid,
                        request_id,
                        "no media session with this peer; open one before sending frames"
                            .to_string(),
                    ),
                    uuid,
                })
            }
        }
    };

    match result {
        // Frames are fire-and-forget: acknowledging every one would put a
        // response on the wire per frame — tens per second per track — for
        // information the sender cannot act on. Only failures are reported.
        Ok(()) => None,
        Err(e) => Some(HandledRequestResult {
            response: failed(cid, peer_cid, request_id, e.to_string()),
            uuid,
        }),
    }
}

pub async fn handle_close<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::MediaClose {
        request_id,
        cid,
        peer_cid,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };

    {
        let mut map = this.server_connection_map.write();
        if let Some(peer) = map
            .get_mut(&cid)
            .and_then(|conn| conn.peers.get_mut(&peer_cid))
        {
            // Dropping the session aborts its inbound pump and releases the UDP
            // halves. The peer connection itself stays up — ending a call must
            // not end the conversation.
            peer.media = None;
        }
    }

    info!(target: "citadel", "[Media] session closed cid={cid} peer_cid={peer_cid}");
    Some(HandledRequestResult {
        response: InternalServiceResponse::MediaSessionClosed(MediaSessionClosed {
            cid,
            peer_cid,
            request_id: Some(request_id),
        }),
        uuid,
    })
}
