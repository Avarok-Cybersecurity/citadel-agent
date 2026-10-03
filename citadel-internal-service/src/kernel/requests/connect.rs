//! C2S Connection Handler
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
//! ### Connect vs Register
//! - `register.rs` → `remote.register()` → Creates NEW account with NEW CID
//! - `connect.rs` (this file) → `remote.connect()` → Connects to EXISTING account, SAME CID

use crate::kernel::reconnect::sign_in::{self, SignIn};
use crate::kernel::reconnect::{LinkState, Reauth};
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::session_route::SessionRoute;
use crate::kernel::sign_in::factors;
use crate::kernel::sign_in::key_relay::Asker;
use crate::kernel::sign_in::{self as pq_sign_in, SessionSignIn};
use crate::kernel::{create_client_server_remote, CitadelWorkspaceService, Connection};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    ConnectFailure, InternalServiceRequest, InternalServiceResponse,
};
use citadel_sdk::prelude::{AuthenticationRequest, ProtocolRemoteExt, Ratchet, SessionScope};
use uuid::Uuid;

pub async fn handle<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::Connect {
        request_id,
        username,
        password,
        security_key,
        recovery_code,
        connect_mode,
        udp_mode,
        keep_alive_timeout,
        session_security_settings,
        server_password,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };
    let connect_mode = crate::kernel::requests::connect_mode::server_connect_mode(
        connect_mode,
        crate::kernel::requests::connect_mode::LoginOrigin::UserSignIn,
    );
    let remote = this.remote();

    // The SDK takes ownership of the password when it carries it to the server,
    // so the fingerprint has to be derived from a copy taken here.
    let password_for_fingerprint = password.clone();

    // GUARD 1: Prevent duplicate concurrent connection attempts for same username
    // This fixes TOCTOU race conditions where two Connect requests arrive simultaneously
    {
        let mut connecting = this.connecting_usernames.lock();
        if connecting.contains(&username) {
            citadel_sdk::logging::warn!(target: "citadel", "[Connect] BLOCKED: Connection already in progress for user {}", username);
            let response = InternalServiceResponse::ConnectFailure(ConnectFailure {
                cid: 0,
                message: format!("Connection already in progress for user {}", username),
                request_id: Some(request_id),
            });
            return Some(HandledRequestResult { response, uuid });
        }
        connecting.insert(username.clone());
    }

    // Helper to cleanup connecting_usernames on function exit
    let cleanup_username = |this: &CitadelWorkspaceService<T, R>, username: &str| {
        this.connecting_usernames.lock().remove(username);
    };

    // GUARD 2: Session reuse check - prevent duplicate SDK sessions for same username
    // This prevents the race condition where ClaimSession + second Connect resets ratchet
    let existing_cid = {
        let lock = this.server_connection_map.read();
        lock.iter()
            .find(|(_, conn)| conn.username == username)
            .map(|(cid, conn)| (*cid, conn.link))
    };

    if let Some((cid, link)) = existing_cid {
        citadel_sdk::logging::info!(target: "citadel", "[Connect] Found existing session {} for user {}, checking SDK...", cid, username);

        // A session the agent is reconnecting is not in the SDK by design, so the SDK is
        // asked only about the others.
        let sdk_holds = match link {
            LinkState::Reconnecting | LinkState::SigningIn => false,
            LinkState::Up | LinkState::Ending => match remote.sessions().await {
                Ok(sessions) => sessions.sessions.iter().any(|sess| sess.cid == cid),
                Err(e) => {
                    // A FAILED query is not an empty answer. This assumed
                    // "inactive", and the branch that assumption reaches is
                    // destructive: it removes the map entry, prunes CID-scoped
                    // state, and then runs the SDK connect against a session the
                    // SDK may still hold -- the ratchet reset the
                    // SessionAlreadyActive branch exists to prevent. Refuse and
                    // let the caller retry; a transient stream error must not
                    // cost the user a live session.
                    citadel_sdk::logging::warn!(target: "citadel", "[Connect] Failed to query SDK sessions: {:?}; refusing rather than assuming the session is gone", e);
                    cleanup_username(this, &username);
                    let response = InternalServiceResponse::ConnectFailure(ConnectFailure {
                        cid,
                        message: format!(
                            "Could not determine whether session {} is still active: {:?}. \
                         Nothing was changed; try again.",
                            cid, e
                        ),
                        request_id: Some(request_id),
                    });
                    return Some(HandledRequestResult { response, uuid });
                }
            },
        };
        let tracked = sign_in::tracked(link, sdk_holds);

        // Prove the caller knows the password before handing them a tracked session. The
        // live branch never reaches the SDK, so nothing else in it ever looks at the
        // password: it used to re-point the session's message stream to the caller and
        // return the real CID on the strength of a username alone. A takeover does reach
        // the SDK, but it stops the reconnect first, so a wrong password must not get
        // that far either.
        //
        // See kernel/credential_fingerprint.rs for why this is a recorded fingerprint and
        // not a local credential check -- the short version is that authentication
        // belongs to the server, re-connecting here would reset the ratchet this branch
        // exists to protect, and the SDK's client-side `validate_credentials` rejects
        // every password.
        let authorized = match tracked {
            sign_in::Tracked::Stale => false,
            sign_in::Tracked::Live | sign_in::Tracked::Reconnecting => {
                let presented = match password_for_fingerprint.clone() {
                    Some(password) => {
                        crate::kernel::credential_fingerprint::derive(remote, &username, password)
                            .await
                    }
                    None => None,
                };
                let lock = this.server_connection_map.read();
                lock.get(&cid).is_some_and(|conn| {
                    crate::kernel::credential_fingerprint::matches(
                        conn.credential_fingerprint.as_ref(),
                        presented.as_ref(),
                    )
                })
            }
        };

        if let Some(refused) = this.refuse_older_connect(authorized, cid, uuid, request_id) {
            cleanup_username(this, &username);
            return Some(refused);
        }
        match sign_in::on_sign_in(tracked, authorized) {
            SignIn::Refuse => {
                citadel_sdk::logging::warn!(target: "citadel", "[Connect] REFUSED reuse of session {} for user {}: the password does not match the one that opened it", cid, username);
                cleanup_username(this, &username);
                // Deliberately the same message a wrong password on a fresh
                // account gets, and no CID: telling the caller that a session
                // exists for this username would make the handler an oracle for
                // who is signed in on this agent.
                let response = InternalServiceResponse::ConnectFailure(ConnectFailure {
                    cid: 0,
                    message: "Invalid username or password".to_string(),
                    request_id: Some(request_id),
                });
                return Some(HandledRequestResult { response, uuid });
            }
            SignIn::AlreadyActive => {
                citadel_sdk::logging::info!(target: "citadel", "[Connect] Session {} already active for user {} - returning SessionAlreadyActive", cid, username);
                crate::kernel::membership::take_over(this, cid, uuid);
                this.host_ilm_for(cid, uuid).await;
                // Lets the frontend handle it gracefully (e.g. redirect to the workspace).
                let response = InternalServiceResponse::SessionAlreadyActive(
                    citadel_internal_service_types::SessionAlreadyActive {
                        cid,
                        username: username.clone(),
                        message: "Session already active. Use the navbar to switch sessions or proceed to workspace.".to_string(),
                        request_id: Some(request_id),
                    },
                );
                cleanup_username(this, &username);
                return Some(HandledRequestResult { response, uuid });
            }
            SignIn::TakeOverReconnect => {
                citadel_sdk::logging::info!(target: "citadel", "[Connect] Session {} for user {} is reconnecting; the sign-in takes it over", cid, username);
                let credentials = crate::kernel::reconnect::Credentials {
                    reauth: pq_sign_in::reauth(SessionScope::Full, false, password),
                    connect_mode,
                    udp_mode,
                    keep_alive_timeout,
                    session_security_settings,
                    server_password,
                    connect_request_id: request_id,
                };
                let response = crate::kernel::reconnect::takeover::take_over(
                    this,
                    cid,
                    uuid,
                    request_id,
                    username.clone(),
                    credentials,
                )
                .await;
                cleanup_username(this, &username);
                return Some(HandledRequestResult { response, uuid });
            }
            SignIn::ReplaceStale => {
                // Internal has session but SDK doesn't - clean up stale state
                citadel_sdk::logging::info!(target: "citadel", "[Connect] Clearing stale session {} for user {} (SDK session disconnected)", cid, username);
                this.server_connection_map.write().remove(&cid);
                this.prune_cid_scoped_state(cid, None);
                // Allow SDK protocol layer to stabilize after stale session cleanup
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }
    }

    // Save username for cleanup (will be moved into SDK connect)
    let username_for_cleanup = username.clone();
    let asker = Asker {
        audience: vec![uuid],
        cid: 0,
        request_id,
    };
    let (challenges, clients) = (&this.key_challenges, &this.tx_to_localhost_clients);
    let offered = factors::begin(
        challenges,
        clients,
        asker,
        password.clone(),
        security_key,
        recovery_code,
    );
    let (factors, underway) = match offered {
        Ok(begun) => begun,
        Err(message) => {
            cleanup_username(this, &username_for_cleanup);
            let response = InternalServiceResponse::ConnectFailure(ConnectFailure {
                cid: 0,
                message,
                request_id: Some(request_id),
            });
            return Some(HandledRequestResult { response, uuid });
        }
    };

    // Proceed with new connection (no existing session or stale session was cleaned)
    match remote
        .connect(
            AuthenticationRequest::sign_in(username, factors),
            connect_mode,
            udp_mode,
            keep_alive_timeout,
            session_security_settings,
            server_password.clone(),
        )
        .await
    {
        Ok(conn_success) => {
            let cid = conn_success.cid;
            citadel_sdk::logging::info!(target: "citadel", "[Connect] SUCCESS: cid={}", cid);

            // A `GetActiveSessions` subscription stood here, between the
            // successful SDK connect and building the `Connection`, so
            // `ConnectSuccess` waited on it. It was labelled DEBUG and its only
            // consumer was the `info!` that printed the result.
            //
            // Its `.next().await` was UNBOUNDED. Every other SDK query in this
            // tree carries a limit -- PEER_LIST_TIMEOUT, PEER_SEND_TIMEOUT, the
            // 30s connect_to_peer_custom -- and a subscription that never
            // yields would have left login permanently unanswered, with the last
            // log line reading "Querying active sessions after connect...",
            // which reads as an SDK connect failure rather than as a discarded
            // debug query.
            //
            // The liveness check that is actually used elsewhere is
            // `remote.sessions()`, which does not go through a subscription.

            let scope = underway.scope;
            let reconnect_credentials = crate::kernel::reconnect::Credentials {
                reauth: underway.finish(password.clone()),
                connect_mode,
                udp_mode,
                keep_alive_timeout,
                session_security_settings,
                server_password,
                connect_request_id: request_id,
            };
            let (sink, stream, handle) = match pq_sign_in::open(conn_success) {
                Ok(opened) => opened,
                Err(err) => {
                    cleanup_username(this, &username_for_cleanup);
                    let response = InternalServiceResponse::ConnectFailure(ConnectFailure {
                        cid,
                        message: err.into_string(),
                        request_id: Some(request_id),
                    });
                    return Some(HandledRequestResult { response, uuid });
                }
            };
            let client_server_remote = create_client_server_remote(
                stream.vconn_type,
                remote.clone(),
                session_security_settings,
            );

            // Refuse, for the same reason the server-address read below refuses.
            //
            // This was `.ok().flatten().unwrap_or_else(|| "#INVALID_USERNAME")`,
            // and the session is RECORDED under whatever this produces. GUARD 2
            // above compares the next Connect's username against
            // `conn.username`, so a session stored as `#INVALID_USERNAME` matches
            // nothing: the guard sees no existing session, and a second SDK
            // connect runs against a live one -- which is the ratchet reset that
            // guard exists to prevent.
            //
            // The fix landed on the `server_address` read fifteen lines down and
            // not on this one, which is the same shape of miss: an unreadable
            // value replaced by a placeholder that every later comparison fails
            // against.
            let username = match remote.account_manager().get_username_by_cid(cid).await {
                Ok(Some(username)) => username,
                Ok(None) | Err(_) => {
                    citadel_sdk::logging::warn!(
                        target: "citadel",
                        "[Connect] Could not read the username for {}; reporting the connect as \
                         failed rather than recording the session under a placeholder no later \
                         request will match",
                        cid
                    );
                    cleanup_username(this, &username_for_cleanup);
                    let response = InternalServiceResponse::ConnectFailure(ConnectFailure {
                        cid,
                        message: format!(
                            "Connected, but could not determine the username for session {}. \
                             Nothing is recorded under a name that would not match; try again.",
                            cid
                        ),
                        request_id: Some(request_id),
                    });
                    return Some(HandledRequestResult { response, uuid });
                }
            };

            // Get server address from the CNAC's connection info.
            //
            // `.ok().flatten()...unwrap_or_default()` turned an unreadable CNAC
            // into an EMPTY address, right after a connect the server had
            // accepted. The UI keys every stored session on
            // `username@serverAddress` -- auto-reconnect, sign-out records and
            // findSessionForServer all do -- so an empty one never matches its
            // stored record: the live session reads as "not active", gets
            // reconnected, is answered SessionAlreadyActive, and the account
            // ends up in the dead state auto-reconnect used to leave behind.
            //
            // Refuse instead. The session is up either way; what we cannot do
            // is report it under a name nothing will match.
            //
            // An account registered to a WebSocket URL is reported under the URL: its
            // CNAC address is one of the HTTP edge's, shared by every workspace behind
            // it, so two sessions on two workspaces would carry the same address.
            let server_address = match remote.server_endpoint(cid).await {
                Ok(Some(endpoint)) => Ok(Some(endpoint.to_string())),
                Ok(None) => remote
                    .account_manager()
                    .get_persistence_handler()
                    .get_cnac_by_cid(cid)
                    .await
                    .map(|cnac| cnac.map(|cnac| cnac.get_connect_info().addr.to_string()))
                    .map_err(|err| err.into_string()),
                Err(err) => Err(err.into_string()),
            };
            let server_address = match server_address {
                Ok(Some(server_address)) => server_address,
                Ok(None) | Err(_) => {
                    citadel_sdk::logging::warn!(
                        target: "citadel",
                        "[Connect] Could not read the server address for {}; reporting the \
                         connect as failed rather than under an address nothing matches",
                        cid
                    );
                    // `username_for_cleanup`, not the shadowed `username`.
                    //
                    // GUARD 1 inserted the REQUEST's username into
                    // `connecting_usernames`; the binding in scope here is the
                    // SDK-derived one that shadows it. Removing the wrong key
                    // leaves the request's username in the set, and GUARD 1 then
                    // refuses that user every subsequent attempt until the agent
                    // restarts. The two other exits below already use it.
                    cleanup_username(this, &username_for_cleanup);
                    let response = InternalServiceResponse::ConnectFailure(ConnectFailure {
                        cid,
                        message: format!(
                            "Connected, but could not determine the server address for session \
                             {}. Nothing is recorded under a name that would not match; try again.",
                            cid
                        ),
                        request_id: Some(request_id),
                    });
                    return Some(HandledRequestResult { response, uuid });
                }
            };

            // best-effort: the host is informational (a label for the account),
            // and a session must not be refused over a label. An unreadable one
            // is reported as absent and logged.
            let server_host = match crate::kernel::server_host::load(remote, cid).await {
                Ok(server_host) => server_host,
                Err(err) => {
                    citadel_sdk::logging::warn!(
                        target: "citadel",
                        "[Connect] Could not read the recorded server host for {}: {}; reporting none",
                        cid, err
                    );
                    None
                }
            };

            // Recorded from the password the SERVER just accepted, so a later
            // reuse request has something to prove itself against -- and only when the
            // password is all the sign-in proved. A session a key or a recovery code opened
            // records none, so neither a reuse nor an attach can reach it on a password.
            let fingerprint = match &reconnect_credentials.reauth {
                Reauth::Password(password) => {
                    crate::kernel::credential_fingerprint::derive(
                        remote,
                        &username,
                        password.clone(),
                    )
                    .await
                }
                Reauth::NeedsUser(_) => None,
            };

            let subscribers = crate::kernel::session_subscribers::SessionSubscribers::new(uuid);
            let connection_struct = Connection::new(
                sink,
                client_server_remote,
                subscribers.clone(),
                username,
                server_address,
                server_host,
                fingerprint,
                reconnect_credentials,
                SessionSignIn { handle, scope },
            );
            {
                let mut map = this.server_connection_map.write();
                map.insert(cid, connection_struct);
                // Signed in again: no longer signed out by the server. Under the map's
                // lock, as the give-up records it (reconnect/report.rs).
                this.signed_out.clear(cid);
            }

            let response = InternalServiceResponse::ConnectSuccess(
                citadel_internal_service_types::ConnectSuccess {
                    cid,
                    request_id: Some(request_id),
                },
            );

            crate::kernel::c2s_reader::spawn(
                SessionRoute::new(subscribers, this.tx_to_localhost_clients.clone()),
                cid,
                stream,
                request_id,
            );

            // A recovery session may not message: the server drops it, so no ILM is hosted.
            if scope == SessionScope::Full {
                this.host_ilm_for(cid, uuid).await;
            }
            cleanup_username(this, &username_for_cleanup);
            Some(HandledRequestResult { response, uuid })
        }

        Err(err) => {
            let response = InternalServiceResponse::ConnectFailure(ConnectFailure {
                cid: 0,
                message: err.into_string(),
                request_id: Some(request_id),
            });

            cleanup_username(this, &username_for_cleanup);
            Some(HandledRequestResult { response, uuid })
        }
    }
}
