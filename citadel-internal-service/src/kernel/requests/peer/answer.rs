//! Answering a peer's stored PeerConnect offer: the one path a window's
//! `PeerConnectAccept` and the agent's own answer (kernel/inbound_connect) share.

use crate::kernel::requests::peer::turn::set_peer_turn;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::PeerTurnConfig;
use citadel_sdk::logging::info;
use citadel_sdk::prelude::{PreSharedKey, Ratchet};
use citadel_sdk::responses;

/// Accept or decline the offer `peer_cid` made to session `cid`.
///
/// An accept when the peer is already connected is a success that sends
/// nothing: duplicate accepts (multi-tab races) must not fail after the first
/// one succeeded. A refusal is never short-circuited that way -- it would be
/// reported delivered while the peer stayed connected.
pub(crate) async fn answer_offer<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    peer_cid: u64,
    accept: bool,
    turn: Option<&PeerTurnConfig>,
    peer_session_password: Option<PreSharedKey>,
) -> Result<(), String> {
    let already_connected = this
        .server_connection_map
        .read()
        .get(&cid)
        .is_some_and(|conn| conn.peers.contains_key(&peer_cid));
    if accept && already_connected {
        info!(target: "citadel", "[PeerConnectAccept] Peer {peer_cid} already connected to {cid} - idempotent success");
        return Ok(());
    }

    let signal = this
        .pending_peer_connect_signals
        .write()
        .remove(&(cid, peer_cid))
        .ok_or_else(|| format!("No pending connection request from peer {peer_cid}"))?;

    let remote = this.remote();
    // The accepting half of the TURN config, set before the accept lets the attempt start. A
    // decline starts no attempt, so it only clears.
    let turn = if accept { turn } else { None };
    set_peer_turn(remote, cid, peer_cid, turn)
        .await
        .map_err(|err| err.into_string())?;

    let ticket = responses::peer_connect(signal, accept, remote, peer_session_password)
        .await
        .map_err(|err| err.into_string())?;
    info!(target: "citadel", "[PeerConnectAccept] Sent {} to {peer_cid} for {cid}, ticket={ticket:?}",
        if accept { "accept" } else { "decline" });
    Ok(())
}

/// The relay config a window last gave for `cid`'s connects (on its PeerConnect
/// or PeerConnectAccept), so the agent's own answer carries the same half a
/// window's would. Memory only, and gone with the session, like the grant.
pub(crate) fn remember_window_relay<T, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    turn: Option<&PeerTurnConfig>,
) {
    if let Some(conn) = this.server_connection_map.write().get_mut(&cid) {
        conn.window_relay = turn.cloned();
    }
}

pub(crate) fn window_relay<T, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
) -> Option<PeerTurnConfig> {
    this.server_connection_map
        .read()
        .get(&cid)
        .and_then(|conn| conn.window_relay.clone())
}
