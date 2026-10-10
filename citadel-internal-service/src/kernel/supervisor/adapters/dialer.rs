//! `PeerDialer` over the same PeerConnect a window's request runs (`requests/peer/dial.rs`),
//! with the session's last TURN config, and with the answer sent to the windows as the
//! request's answer would have been.

use crate::kernel::requests::peer::answer::window_relay;
use crate::kernel::requests::peer::dial::{dial, Dial, ALREADY_CONNECTED};
use crate::kernel::session_route::SessionRoute;
use crate::kernel::supervisor::ports::PeerDialer;
use crate::kernel::supervisor::types::{Cid, DialOutcome};
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::InternalServiceResponse;
use citadel_sdk::logging::info;
use citadel_sdk::prelude::{Ratchet, UdpMode};
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
            // The security settings the session was opened with: no window is asking, so
            // none is the source of these.
            let opened = this.server_connection_map.read().get(&cid).map(|conn| {
                (
                    conn.reconnect.session_security_settings,
                    conn.subscribers.clone(),
                )
            });
            let Some((session_security_settings, subscribers)) = opened else {
                return DialOutcome::Failed;
            };
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
            let outcome = match &response {
                InternalServiceResponse::PeerConnectSuccess(_) => DialOutcome::Connected,
                InternalServiceResponse::PeerConnectFailure(failure)
                    if failure.message.starts_with(ALREADY_CONNECTED) =>
                {
                    DialOutcome::Connected
                }
                _ => DialOutcome::Failed,
            };
            info!(target: "citadel::supervisor", "{cid}: dialling {peer}: {outcome:?}");
            if outcome == DialOutcome::Connected {
                // Windows learn of it as they do of an accepted connection.
                SessionRoute::new(subscribers, this.tx_to_localhost_clients.clone()).send(response);
            } else {
                info!(target: "citadel::supervisor", "{cid}: dialling {peer} failed: {response:?}");
            }
            outcome
        }
        .boxed()
    }
}
