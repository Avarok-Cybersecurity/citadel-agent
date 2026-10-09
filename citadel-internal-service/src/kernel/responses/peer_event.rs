//! P2P (Peer-to-Peer) Event Handler
//!
//! This module handles SDK `NodeResult::PeerEvent` events, including:
//! - `PeerSignal::Disconnect`: P2P connection terminated
//! - `PeerSignal::PostRegister`: Peer registration request received
//! - `PeerSignal::PostConnect`: Peer connection request received
//! - `PeerSignal::BroadcastConnected`: Group broadcast event
//!
//! ## P2P Disconnect Flow (PeerSignal::Disconnect)
//! 1. Either peer calls `remote.find_target(cid, peer_cid).disconnect()`
//! 2. SDK sends `PeerSignal::Disconnect`, waits for `PeerEvent(PeerSignal::Disconnect)`
//! 3. This handler cleans up internal service state (removes peer from session)
//! 4. Notifies TCP client via `DisconnectNotification`
//!
//! ## Distinction from C2S Disconnect
//! - C2S: `NodeResult::Disconnect` - entire session terminated
//! - P2P: `PeerSignal::Disconnect` - single peer connection terminated
//!
//! Both remove the peer through `requests/peer/disconnect.rs`; a report only when it names the connection the peer has now.

use crate::kernel::reconnect::instance::PeerEnd;
use crate::kernel::requests::peer::cleanup_reported_peer;
use crate::kernel::session_route::SessionRoute;
use crate::kernel::session_subscribers::SessionSubscribers;
use crate::kernel::supervisor::Signal;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    DisconnectNotification, InternalServiceResponse, PeerConnectNotification,
    PeerRegisterNotification,
};
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::{
    GroupEvent, NetworkError, PeerConnectionType, PeerEvent, PeerSignal, Ratchet,
};

#[cfg(test)]
mod tests;

/// Deliver a peer notification to every connection attached to its session.
///
/// Resolved through the CID at send time, never broadcast: the fallback to every
/// active localhost connection this once had handed every account on the agent
/// the peer-register, peer-connect and disconnect notifications of every other.
/// `removed` is the subscriber set of an entry that has just left the map (a P2P
/// disconnect cleans up before it notifies); otherwise the live set is used.
/// Nobody attached is a drop with a warning, as everywhere else.
async fn send_response_for_session<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    response: InternalServiceResponse,
    session_cid: u64,
    removed: Option<SessionSubscribers>,
) -> Result<(), NetworkError> {
    this.notice_for(&response);
    let subscribers = crate::kernel::membership::subscribers_of(this, session_cid).or(removed);
    let delivered = subscribers
        .map(|subs| SessionRoute::new(subs, this.tx_to_localhost_clients.clone()).send(response))
        .unwrap_or_default();
    if delivered.is_empty() {
        warn!(target: "citadel", "No localhost connection attached to CID {session_cid} - peer notification dropped");
    }
    Ok(())
}

pub async fn handle<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    event: PeerEvent,
) -> Result<(), NetworkError> {
    match event.event {
        PeerSignal::Disconnect {
            peer_conn_type:
                PeerConnectionType::LocalGroupPeer {
                    session_cid,
                    peer_cid,
                },
            disconnect_token,
            ..
        } => {
            // SDK is source of truth - clean up P2P peer state to mirror SDK
            info!(
                target: "citadel",
                "[P2P Disconnect] SDK reports peer {} disconnected from session {} - cleaning up internal state",
                peer_cid,
                session_cid
            );

            // Use shared cleanup function (DRY)
            // NOTE: For SDK-initiated P2P disconnect events, the SDK has already disconnected.
            // We just remove from our map and let the struct drop (RAII is harmless).
            if let Some(disconnected) = cleanup_reported_peer(
                &this.server_connection_map,
                session_cid,
                peer_cid,
                PeerEnd::Reported(disconnect_token.map(|token| token.connection_id)),
            )
            .inspect(|_| {
                this.prune_cid_scoped_state(session_cid, Some(peer_cid));
                this.supervisors
                    .signal(session_cid, Signal::PeerLost(peer_cid));
            }) {
                let subscribers = disconnected.subscribers().clone();
                // Let the struct drop - SDK already disconnected so RAII is harmless
                drop(disconnected);

                let response =
                    InternalServiceResponse::DisconnectNotification(DisconnectNotification {
                        cid: session_cid,
                        peer_cid: Some(peer_cid),
                        request_id: None,
                        ended_locally: None,
                    });
                // Re-resolved through the CID, never broadcast; see the function.
                send_response_for_session(this, response, session_cid, Some(subscribers)).await?;
            }
        }
        PeerSignal::BroadcastConnected {
            session_cid,
            group_broadcast,
        } => {
            let evt = GroupEvent {
                session_cid,
                ticket: event.ticket,
                event: group_broadcast,
            };
            return super::group_event::handle(this, evt).await;
        }
        PeerSignal::PostRegister {
            peer_conn_type:
                PeerConnectionType::LocalGroupPeer {
                    session_cid: peer_cid,
                    peer_cid: session_cid,
                },
            inviter_username,
            invitee_username: _,
            ticket_opt: _,
            invitee_response: _,
        } => {
            info!(target: "citadel", "User {session_cid:?} received Register Request from {peer_cid:?}");

            // Cache the peer's username for later use in ListRegisteredPeers
            // The SDK's get_local_group_mutual_peers may not return usernames
            if !inviter_username.is_empty() {
                let mut cache = this.peer_username_cache.write();
                cache.insert((session_cid, peer_cid), inviter_username.clone());
                info!(target: "citadel", "Cached username '{}' for peer {} (session {})", inviter_username, peer_cid, session_cid);
            }

            // Store the pending signal for later acceptance via PeerRegisterRespond
            // The signal is stored with the original structure (CIDs as received from SDK)
            // The responses::peer_register() function will handle the reversal
            let pending_signal = PeerSignal::PostRegister {
                peer_conn_type: PeerConnectionType::LocalGroupPeer {
                    session_cid: peer_cid,
                    peer_cid: session_cid,
                },
                inviter_username: inviter_username.clone(),
                invitee_username: None,
                ticket_opt: Some(event.ticket),
                invitee_response: None,
            };
            {
                let mut signals = this.pending_peer_registrations.write();
                signals.insert((session_cid, peer_cid), pending_signal);
                info!(target: "citadel", "[PostRegister] Stored pending registration signal for (cid={}, peer_cid={}), total pending: {}", session_cid, peer_cid, signals.len());
            }

            {
                let response =
                    InternalServiceResponse::PeerRegisterNotification(PeerRegisterNotification {
                        cid: session_cid,
                        peer_cid,
                        peer_username: inviter_username,
                        request_id: None,
                    });
                // Re-resolved through the CID, never broadcast; see the function.
                send_response_for_session(this, response, session_cid, None).await?;
            }
        }
        PeerSignal::PostConnect {
            peer_conn_type:
                PeerConnectionType::LocalGroupPeer {
                    session_cid: peer_cid,
                    peer_cid: session_cid,
                },
            ticket_opt: _,
            invitee_response: Some(response),
            ..
        } => {
            // An answer to OUR dial, not an offer: its outcome already went to the dial's
            // PeerConnect request. Reaches here when the SDK forwards it after the dial's
            // listener has gone (see peer_event/tests.rs).
            info!(target: "citadel", "User {session_cid:?}: {peer_cid:?} answered our PeerConnect ({response:?}); not an incoming request");
        }
        PeerSignal::PostConnect {
            peer_conn_type:
                PeerConnectionType::LocalGroupPeer {
                    session_cid: peer_cid,
                    peer_cid: session_cid,
                },
            ticket_opt: _,
            invitee_response: None,
            session_security_settings,
            udp_mode,
            session_password: _,
        } => {
            info!(target: "citadel", "User {session_cid:?} received Connect Request from {peer_cid:?}");

            // Store the pending signal for later acceptance via PeerConnectAccept
            // We reconstruct the signal since the match consumes the fields
            let pending_signal = PeerSignal::PostConnect {
                peer_conn_type: PeerConnectionType::LocalGroupPeer {
                    // Note: The original signal has session_cid/peer_cid swapped from our perspective
                    session_cid: peer_cid,
                    peer_cid: session_cid,
                },
                ticket_opt: Some(event.ticket),
                invitee_response: None,
                session_security_settings,
                udp_mode,
                session_password: None,
            };
            {
                let mut signals = this.pending_peer_connect_signals.write();
                signals.insert((session_cid, peer_cid), pending_signal);
                info!(target: "citadel", "[PostConnect] Stored pending PeerConnect signal for (cid={}, peer_cid={}), total pending: {}", session_cid, peer_cid, signals.len());
            }

            // Decided before windows hear of it: the notification says whether the
            // agent answers (kernel/inbound_connect), so no window answers too.
            // Off the event loop, as a window's answer is.
            let this = this.clone();
            drop(tokio::spawn(async move {
                let answer = this
                    .agent_answer_for(
                        session_cid,
                        peer_cid,
                        session_security_settings.security_level,
                    )
                    .await;
                let response =
                    InternalServiceResponse::PeerConnectNotification(PeerConnectNotification {
                        cid: session_cid,
                        peer_cid,
                        session_security_settings,
                        udp_mode,
                        request_id: None,
                        answered_by_agent: answer.agent_has_it(),
                    });
                // Re-resolved through the CID, never broadcast; see the function.
                if let Err(err) =
                    send_response_for_session(&this, response, session_cid, None).await
                {
                    warn!(target: "citadel", "[PostConnect] notification for {session_cid} not sent: {err:?}");
                }
                this.answer_as_agent(session_cid, peer_cid, answer).await;
            }));
        }
        _ => {}
    }

    Ok(())
}
