//! `PeerDialer` over the same PeerConnect a window's request runs (`requests/peer/dial.rs`),
//! with the session's last TURN config, and with the answer sent to the windows as the
//! request's answer would have been.

use crate::kernel::conversations::engine::preferences;
use crate::kernel::requests::peer::answer::window_relay;
use crate::kernel::requests::peer::dial::{dial, Dial, ALREADY_CONNECTED};
use crate::kernel::session_route::SessionRoute;
use crate::kernel::supervisor::ports::PeerDialer;
use crate::kernel::supervisor::types::{Cid, DialOutcome};
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{InternalServiceResponse, SessionSecuritySettings};
use citadel_sdk::logging::{debug, info, warn};
use citadel_sdk::prelude::{Ratchet, SecurityLevel, UdpMode};
use futures::future::BoxFuture;
use futures::FutureExt;

/// Peers are dialled with UDP, as a window dials them: a call's media needs the datagram path.
/// Not the session's own mode, which is the server link's (opened with UDP off by the browser):
/// copying that brought every supervised peer up unable to carry a call.
const PEER_UDP_MODE: UdpMode = UdpMode::Enabled;

pub(super) struct KernelDialer<T, R: Ratchet> {
    pub this: CitadelWorkspaceService<T, R>,
    pub cid: u64,
}

impl<T: IOInterface + Sync, R: Ratchet> PeerDialer for KernelDialer<T, R> {
    fn dial(&self, peer: Cid) -> BoxFuture<'static, DialOutcome> {
        let (this, cid) = (self.this.clone(), self.cid);
        async move {
            // The security settings the session was opened with, at the chat's level: no window
            // is asking, so none is the source of these.
            let opened = this.server_connection_map.read().get(&cid).map(|conn| {
                (
                    conn.reconnect.session_security_settings,
                    conn.subscribers.clone(),
                )
            });
            let Some((opened, subscribers)) = opened else {
                return DialOutcome::Failed;
            };
            // The chat's level, which the window would have dialled at. Unreadable is no dial:
            // guessing Standard is quietly lowering the chat, the one wrong answer.
            let minimum = match preferences(&this, cid).await {
                Ok(prefs) => prefs.security_minimum_for(peer).sdk(),
                Err(err) => {
                    warn!(target: "citadel::supervisor", "{cid}: not dialling {peer}: preferences unreadable ({err})");
                    return DialOutcome::Failed;
                }
            };
            let session_security_settings = chat_dial_settings(opened, minimum);
            info!(target: "citadel::supervisor", "{cid}: dialling {peer}");
            let response = dial(
                &this,
                Dial {
                    cid,
                    peer_cid: peer,
                    udp_mode: PEER_UDP_MODE,
                    session_security_settings,
                    peer_session_password: None,
                    turn: window_relay(&this, cid),
                    request_id: None,
                },
            )
            .await;
            let outcome = outcome_of(&response);
            info!(target: "citadel::supervisor", "{cid}: dialling {peer}: {outcome:?}");
            if outcome == DialOutcome::Connected {
                // Windows learn of it as they do of an accepted connection.
                SessionRoute::new(subscribers, this.tx_to_localhost_clients.clone()).send(response);
            } else {
                debug!(target: "citadel::supervisor", "{cid}: dialling {peer}: {response:?}");
            }
            outcome
        }
        .boxed()
    }
}

/// What a supervised dial opens the channel with: the session's settings, at the chat's
/// minimum when that is higher. The session's level alone reopened a chat the user had just
/// raised at Standard, while this agent declined the peer's own Standard offers for being
/// below it (owner report 2026-11-15). Never lower than the session's.
pub(super) fn chat_dial_settings(
    opened: SessionSecuritySettings,
    minimum: SecurityLevel,
) -> SessionSecuritySettings {
    if minimum.value() > opened.security_level.value() {
        SessionSecuritySettings {
            security_level: minimum,
            ..opened
        }
    } else {
        opened
    }
}

/// The SDK's refusal of a second key exchange while the first runs (protocol #372). It is
/// wrapped in the SDK's own prefix, so it is looked for, not matched at the start.
pub(super) const ALREADY_EXCHANGING_KEYS: &str = "Already exchanging keys with peer";

pub(super) fn outcome_of(response: &InternalServiceResponse) -> DialOutcome {
    match response {
        InternalServiceResponse::PeerConnectSuccess(_) => DialOutcome::Connected,
        InternalServiceResponse::PeerConnectFailure(failure)
            if failure.message.starts_with(ALREADY_CONNECTED) =>
        {
            DialOutcome::Connected
        }
        InternalServiceResponse::PeerConnectFailure(failure)
            if failure.message.contains(ALREADY_EXCHANGING_KEYS) =>
        {
            DialOutcome::InProgress
        }
        _ => DialOutcome::Failed,
    }
}
