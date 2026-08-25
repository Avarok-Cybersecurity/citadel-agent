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

use crate::kernel::media::{self, PeerMediaSession, UdpState};
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
    // Ownership is checked here, not only at open. A reconnect replaces the
    // session while the previous localhost connection may still have frames in
    // flight; without this, those late frames are injected into the NEW call,
    // which the new owner neither started nor can see the source of.
    let lookup = {
        let map = this.server_connection_map.read();
        map.get(&cid)
            .and_then(|conn| conn.peers.get(&peer_cid))
            .and_then(|peer| peer.media.as_ref())
            .map(|session| (session.owner(), session.outbound()))
    };

    let outbound = match lookup {
        Some((owner, outbound)) if owner == uuid => outbound,
        Some(_) => {
            return Some(HandledRequestResult {
                response: failed(
                    cid,
                    peer_cid,
                    request_id,
                    "this media session belongs to another connection; open your own before \
                     sending frames"
                        .to_string(),
                ),
                uuid,
            });
        }
        None => {
            return Some(HandledRequestResult {
                response: failed(
                    cid,
                    peer_cid,
                    request_id,
                    "no media session with this peer; open one before sending frames".to_string(),
                ),
                uuid,
            });
        }
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

    enum Close<R: Ratchet> {
        /// Nothing here belongs to this connection, so nothing may be disturbed.
        NotYours,
        Took(Option<Box<PeerMediaSession<R>>>),
    }

    let outcome = {
        let mut map = this.server_connection_map.write();
        match map
            .get_mut(&cid)
            .and_then(|conn| conn.peers.get_mut(&peer_cid))
        {
            None => Close::Took(None),
            Some(peer) => {
                let authorised = close_authorised(
                    peer.media.as_ref().map(|session| session.owner()),
                    peer.media_pending_owner,
                    uuid,
                );

                if authorised {
                    // Bumped even when no session exists yet: an open may be
                    // mid-await, and without this it would install a session
                    // into a call the client just ended (a zombie pump
                    // streaming frames forever). The open compares generations
                    // before committing.
                    peer.media_generation += 1;
                    peer.media_pending_owner = None;
                    Close::Took(peer.media.take())
                } else {
                    Close::NotYours
                }
            }
        }
    };

    let Close::Took(session) = outcome else {
        return Some(HandledRequestResult {
            response: failed(
                cid,
                peer_cid,
                request_id,
                "this media session belongs to another connection".to_string(),
            ),
            uuid,
        });
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

/// Whether `requester` may tear down whatever media state a peer currently has.
///
/// Split out from `handle_close` because the interesting cases are all about
/// which uuid holds the session, and reaching them through the real handler
/// would mean constructing a whole service, a peer connection and a live UDP
/// path to assert one boolean.
///
/// An established session names its owner outright. With no session yet, the
/// only thing a close can affect is an open still awaiting its channel, so that
/// open's owner decides. With neither, there is nothing to cancel and the
/// generation bump is a no-op, so it costs nothing to allow.
pub(crate) fn close_authorised(
    session_owner: Option<Uuid>,
    pending_owner: Option<Uuid>,
    requester: Uuid,
) -> bool {
    match (session_owner, pending_owner) {
        (Some(owner), _) => owner == requester,
        (None, Some(pending)) => pending == requester,
        (None, None) => true,
    }
}

#[cfg(test)]
mod authorisation_tests {
    use super::close_authorised;
    use uuid::Uuid;

    /// The case the reconnect bug turns on: the client's uuid is replaced, the
    /// new connection opens a call, and a `MediaClose` from the dead connection
    /// finally arrives. It must not end a call it does not own.
    #[test]
    fn a_stale_connection_cannot_close_the_new_owners_session() {
        let old_owner = Uuid::new_v4();
        let new_owner = Uuid::new_v4();

        assert!(!close_authorised(Some(new_owner), None, old_owner));
        assert!(close_authorised(Some(new_owner), None, new_owner));
    }

    /// Same race, one step earlier: the new owner's open is still awaiting its
    /// UDP channel, so there is no session yet -- only the pending claim stands
    /// between a stale close and a cancelled call.
    #[test]
    fn a_stale_connection_cannot_cancel_the_new_owners_pending_open() {
        let old_owner = Uuid::new_v4();
        let new_owner = Uuid::new_v4();

        assert!(!close_authorised(None, Some(new_owner), old_owner));
        assert!(close_authorised(None, Some(new_owner), new_owner));
    }

    /// A live session outranks a stale claim: the claim is cleared on commit,
    /// but if one were ever left behind it must not grant authority over a
    /// session that names someone else.
    #[test]
    fn the_established_session_owner_wins_over_a_leftover_claim() {
        let owner = Uuid::new_v4();
        let stale_claim = Uuid::new_v4();

        assert!(!close_authorised(
            Some(owner),
            Some(stale_claim),
            stale_claim
        ));
        assert!(close_authorised(Some(owner), Some(stale_claim), owner));
    }

    /// Closing when nothing is open stays a harmless no-op rather than an
    /// error, so an idempotent hang-up from any connection still succeeds.
    #[test]
    fn closing_nothing_is_allowed() {
        assert!(close_authorised(None, None, Uuid::new_v4()));
    }
}
