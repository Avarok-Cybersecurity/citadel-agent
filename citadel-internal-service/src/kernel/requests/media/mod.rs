//! Media session request handlers: open, send, close.
//!
//! The transport itself lives in `kernel::media`; this is the request plumbing
//! that owns the session's lifetime inside a peer connection. The invariant
//! all three handlers protect: the peer's UDP halves are issued once per
//! connection, so they are lent to sessions and re-parked on close, never
//! consumed. `media_generation` arbitrates open/close races — every close bumps
//! it, and an open that awaited must see its captured value unchanged before it
//! may install a session.

mod commit;
mod open;

pub use open::handle_open;

use crate::kernel::media::{self, UdpState};
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, MediaSessionClosed, MediaSessionFailed,
    MediaSessionOpened,
};
use citadel_sdk::logging::info;
use citadel_sdk::prelude::{PeerChannelRecvHalf, Ratchet};
use uuid::Uuid;

pub(crate) fn failed(
    cid: u64,
    peer_cid: u64,
    request_id: Uuid,
    message: String,
) -> InternalServiceResponse {
    InternalServiceResponse::MediaSessionFailed(MediaSessionFailed {
        cid,
        peer_cid,
        message,
        request_id: Some(request_id),
    })
}

pub(crate) fn opened(cid: u64, peer_cid: u64, request_id: Uuid) -> InternalServiceResponse {
    InternalServiceResponse::MediaSessionOpened(MediaSessionOpened {
        cid,
        peer_cid,
        unreliable: true,
        max_frame_bytes: media::MAX_FRAME_BYTES,
        request_id: Some(request_id),
    })
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

    // Frames arrive 30-60 times a second per sender; taking the global write
    // lock for each one made every frame contend with every other handler.
    // Instead, a read lock fetches the session's outbound handle, and the
    // per-session mutex serializes only the packetizer. No await under either.
    let outbound = {
        let map = this.server_connection_map.read();
        map.get(&cid)
            .and_then(|conn| conn.peers.get(&peer_cid))
            .and_then(|peer| peer.media.as_ref())
            .map(|session| session.outbound())
    };

    let Some(outbound) = outbound else {
        return Some(HandledRequestResult {
            response: failed(
                cid,
                peer_cid,
                request_id,
                "no media session with this peer; open one before sending frames".to_string(),
            ),
            uuid,
        });
    };

    let result = outbound
        .lock()
        .send_frame(track, kind, timestamp, flags, payload);
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

    let session = {
        let mut map = this.server_connection_map.write();
        map.get_mut(&cid)
            .and_then(|conn| conn.peers.get_mut(&peer_cid))
            .and_then(|peer| {
                // Bumped even when no session exists yet: an open may be
                // mid-await, and without this it would install a session into a
                // call the client just ended (a zombie pump streaming frames
                // forever). The open compares generations before committing.
                peer.media_generation += 1;
                peer.media.take()
            })
    };

    if let Some(session) = session {
        // The peer connection itself stays up — ending a call must not end the
        // conversation. The recovered receive half is re-parked so the NEXT
        // call on this connection can open (the SDK offers the channel once).
        let recovered = session.close().await;
        park_recovered_receive_half(this, cid, peer_cid, recovered);
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

/// Re-parks the receive half recovered from a closed session next to the send
/// half retained in `UdpState::Lent`, restoring `Idle` for the next call.
pub(crate) fn park_recovered_receive_half<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    peer_cid: u64,
    recovered: Option<PeerChannelRecvHalf<R>>,
) {
    let mut map = this.server_connection_map.write();
    // Peer gone: dropping the half tears down a UDP path nobody can use anyway.
    let Some(peer) = map
        .get_mut(&cid)
        .and_then(|conn| conn.peers.get_mut(&peer_cid))
    else {
        return;
    };
    if let UdpState::Lent { tx } = &peer.udp {
        peer.udp = match recovered {
            Some(rx) => UdpState::Idle { tx: tx.clone(), rx },
            // The pump saw the UDP stream end; there is no path left to lend.
            None => UdpState::Unavailable,
        };
    }
    // Any other state means a fresh handshake replaced the transport while the
    // session was closing; the recovered half belongs to the old path and is
    // correctly dropped here.
}
