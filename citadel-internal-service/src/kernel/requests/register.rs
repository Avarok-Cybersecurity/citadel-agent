//! C2S Registration Handler
//!
//! ## Protocol Semantics (CRITICAL)
//!
//! ### C2S (Client-to-Server)
//! - **Registration**: ONE-TIME per user. Creates permanent CID. Persisted in backend.
//! - **Connection**: Can happen MANY TIMES after registration. Reuses existing CID.
//! - **No re-registration**: The protocol has NO notion of re-registering a user.
//!
//! ### P2P (Peer-to-Peer)
//! - **Registration**: ONE-TIME per peer pair. Consent to communicate. Persisted.
//! - **Connection**: Can happen MANY TIMES after P2P registration.
//! - **No re-registration**: The protocol has NO notion of re-registering peers.
//!
//! ### Key Insight
//! If a user gets a NEW CID after reconnection, it means a NEW ACCOUNT was registered.
//! CID is PERMANENT per account - not per session.
//!
//! ### Register vs Connect
//! - `register.rs` (this file) → `remote.register()` → Creates NEW account with NEW CID
//! - `connect.rs` → `remote.connect()` → Connects to EXISTING account, SAME CID

use crate::kernel::requests::{handle_request, HandledRequestResult};
use crate::kernel::session_route::deliver;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, RecoveryCodes,
};
use citadel_sdk::logging::info;
use citadel_sdk::prelude::{ConnectMode, ProtocolRemoteExt, Ratchet};
use uuid::Uuid;

pub async fn handle<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::Register {
        request_id,
        server_addr,
        full_name,
        username,
        proposed_password,
        connect_after_register,
        session_security_settings,
        server_password,
        admission_token,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };
    let remote = this.remote();

    // Resolve HERE, not in the browser.
    //
    // `server_addr` arrives as `host:port`, a hosted workspace host, or a
    // ws(s):// URL. It used to be a `SocketAddr`, which forced the page to
    // resolve a hostname itself -- with a DNS-over-HTTPS fetch to
    // `https://dns.google/resolve`. A hosted UI's Content-Security-Policy
    // refuses that connection, so on work.avarok.net every hostname address
    // timed out after 30 seconds while a raw IP worked, and where it did work it
    // disclosed each user's server to a third party.
    //
    // A failed lookup is answered, not logged: registration is a foreground
    // action and "Registration timed out" is what the user saw for a name that
    // simply does not resolve.
    let refuse = |message: String| {
        Some(HandledRequestResult {
            response: InternalServiceResponse::RegisterFailure(
                citadel_internal_service_types::RegisterFailure {
                    cid: 0,
                    message,
                    request_id: Some(request_id),
                    reason_code: None,
                },
            ),
            uuid,
        })
    };

    let address = match crate::kernel::server_address::classify(&server_addr) {
        Err(err) => return refuse(format!("invalid server address {server_addr}: {err}")),
        Ok(address) => address,
    };
    // Validated before anything is dialled: an address whose host cannot be
    // recorded is refused rather than registered under a host nothing reports.
    let server_host = match crate::kernel::server_host::from_typed(&address) {
        Err(err) => return refuse(format!("invalid server address {server_addr}: {err}")),
        Ok(server_host) => server_host,
    };

    let registered = match address {
        crate::kernel::server_address::ServerAddress::WebSocket(endpoint) => {
            info!(target: "citadel", "About to register to {endpoint} for user {username}");
            remote
                .register_to_endpoint_admitted(
                    endpoint,
                    full_name,
                    username.clone(),
                    proposed_password.clone(),
                    session_security_settings,
                    server_password.clone(),
                    admission_token,
                )
                .await
        }
        crate::kernel::server_address::ServerAddress::HostPort(host_port) => {
            let server_addr = match tokio::net::lookup_host(&host_port).await {
                Ok(mut addrs) => match addrs.next() {
                    Some(addr) => addr,
                    None => return refuse(format!("{host_port} resolved to no addresses")),
                },
                Err(err) => return refuse(format!("could not resolve {host_port}: {err}")),
            };
            info!(target: "citadel", "About to connect to server {server_addr:?} for user {username}");
            remote
                .register_admitted(
                    server_addr,
                    full_name,
                    username.clone(),
                    proposed_password.clone(),
                    session_security_settings,
                    server_password.clone(),
                    admission_token,
                )
                .await
        }
    };

    if let Ok(res) = &registered {
        // Before the connect below, so the session it opens reports the host.
        //
        // Logged, not answered as a failure: the server has already created the
        // account, and a RegisterFailure would send the user to register again
        // into "username taken". The account works without the label.
        if let Err(err) = crate::kernel::server_host::store(remote, res.cid, &server_host).await {
            citadel_sdk::logging::warn!(
                target: "citadel",
                "[Register] Registered {} but could not record its server host {}: {}",
                res.cid, server_host, err
            );
        }
    }

    match registered {
        Ok(res) => {
            // The codes exist only here: the SDK derived them on this client, and the server
            // keeps only their keys. They go to the window that registered, once, and nowhere
            // else -- not to a log, not to the store.
            let response = InternalServiceResponse::RegisterSuccess(
                citadel_internal_service_types::RegisterSuccess {
                    cid: res.cid,
                    request_id: Some(request_id),
                    recovery_codes: RecoveryCodes(res.recovery_codes),
                },
            );
            if !connect_after_register {
                return Some(HandledRequestResult { response, uuid });
            }
            // Sent ahead of the connect's own answer, which keeps the request id.
            if deliver(&this.tx_to_localhost_clients, &[uuid], response).is_empty() {
                citadel_sdk::logging::warn!(target: "citadel", "[Register] {} registered, but its window left before the recovery codes reached it", res.cid);
            }
            let connect_command = InternalServiceRequest::Connect {
                username,
                password: Some(proposed_password),
                security_key: false,
                recovery_code: None,
                keep_alive_timeout: None,
                udp_mode: Default::default(),
                // The Connect handler sets force_login by origin (requests/connect_mode.rs).
                connect_mode: ConnectMode::Standard { force_login: false },
                session_security_settings,
                request_id,
                server_password,
                admission_token: None,
            };

            handle_request(this, uuid, connect_command).await
        }
        Err(err) => {
            let response = InternalServiceResponse::RegisterFailure(
                citadel_internal_service_types::RegisterFailure {
                    cid: 0,
                    reason_code: crate::kernel::sign_in::failure_reason(err.code),
                    message: err.into_string(),
                    request_id: Some(request_id),
                },
            );

            Some(HandledRequestResult { response, uuid })
        }
    }
}
