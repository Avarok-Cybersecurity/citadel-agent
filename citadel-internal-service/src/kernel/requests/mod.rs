use crate::kernel::CitadelWorkspaceService;
use async_recursion::async_recursion;
use citadel_internal_service_types::*;
use citadel_sdk::logging::info;
use citadel_sdk::logging::tracing::log;

use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_sdk::prelude::*;
use futures::stream::FuturesOrdered;
use futures::StreamExt;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

pub(crate) struct HandledRequestResult {
    pub response: InternalServiceResponse,
    pub uuid: Uuid,
}

mod connect;
mod deregister;
mod disconnect;
mod get_account_information;
mod get_sessions;
mod media;
mod message;
mod register;

mod connection_management;
pub(crate) mod file;
mod group;
mod local_db;
pub(crate) mod peer;

#[async_recursion]
#[allow(clippy::multiple_bound_locations)]
pub async fn handle_request<T, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    command: InternalServiceRequest,
) -> Option<HandledRequestResult>
where
    T: IOInterface + Sync,
{
    // A request may only act on a session THIS connection owns.
    //
    // Every handler used to read `cid` straight off the wire and act on it,
    // while the connection's own identity sat unused in scope — so a request
    // could name any session it liked. `deregister` would destroy that account,
    // `local_db` would read or wipe its store, `message` would send as them.
    // WebSocket is exempt from CORS and the dev stack binds all interfaces, so
    // that was reachable from any page a user happened to visit.
    //
    // `session_cid()` returns None for the six variants that legitimately
    // precede or span a session (Connect, Register, GetSessions,
    // GetAccountInformation, ConnectionManagement, Batched), and those are
    // unaffected. A cid that is not in the map is also let through: the handler
    // owns that error and already reports it, and pre-empting it here would
    // change existing behaviour for clients acting before a session exists.
    // LocalDBGetKV is exempt, on evidence rather than assumption.
    //
    // Enabling this gate produced refusals in ordinary two-peer messaging, and
    // naming the variant showed every one of them was LocalDBGetKV: ILM's
    // messenger backend reads key/value state under a session this connection
    // does not own. Refusing those does not merely log noise — it would break
    // messaging state, and the suite would not have told me, because the client
    // retries and every spec still passed.
    //
    // So the read stays allowed until that access is understood well enough to
    // scope it properly (either ILM keys these reads by the local session, or
    // the gate learns which peer-scoped reads are legitimate). The destructive
    // and impersonating operations — Deregister, Disconnect, Message, SendFile,
    // the LocalDB writes and clears — are gated now, and they are what made this
    // reachable from any page a user visits.
    //
    // Recorded in docs/ROBUSTNESS.md as an open question rather than left as a
    // silent hole in the check.
    // The exemption is now the KEY, not the whole variant.
    //
    // It used to be `matches!(command, LocalDBGetKV { .. })`, which let any
    // connection read ANY key of any account it could name a cid for — and a cid
    // is a u64 that travels in peer lists and notifications, not a secret. The
    // evidence behind the exemption was specific: every refusal came from ILM's
    // messenger backend, and every key ILM touches is one of seven fixed names
    // suffixed with `-{cid}`. Scoping to those preserves exactly the access the
    // evidence justified and withdraws the rest.
    let exempt = is_exempt_from_ownership_gate(&command);

    // Writes must be OWNED, not merely unopposed.
    //
    // The gate lets an unmapped cid through, on the stated grounds that "the
    // handler owns that error and already reports it". That is true of download
    // and delete_virtual_file, which look the cid up in the map and fail. It is
    // NOT true of the LocalDB handlers: they resolve through `propose_target`,
    // which by its own doc only checks that the cid names a locally-known
    // account — not that the caller owns it. So for an account that is known but
    // has no mapped session (after a Disconnect, say), any connection could
    // write or wipe its persistent store.
    let requires_ownership = requires_owned_session(&command);

    if let Some(cid) = command.session_cid().filter(|_| !exempt) {
        let owner = {
            let map = this.server_connection_map.read();
            map.get(&cid).map(|conn| {
                conn.associated_localhost_connection
                    .load(std::sync::atomic::Ordering::Relaxed)
            })
        };
        if owner.is_none() && requires_ownership {
            log::warn!(target: "citadel",
                "Refusing a LocalDB write for session {cid} from connection {uuid}: no mapped session, so ownership cannot be established");
            return None;
        }
        if let Some(owner) = owner {
            if owner != uuid {
                // Name the request type: "something was refused" is not
                // actionable, and the first run of this gate produced 48
                // refusals whose source could not be identified from the log.
                //
                // Variant name only — the Debug output is truncated at the
                // first brace so payloads (message bodies, file contents, KV
                // values) never reach the log.
                let debug = format!("{command:?}");
                let variant = debug.split(['{', '(']).next().unwrap_or("Request").trim();
                log::warn!(target: "citadel",
                    "Refusing {variant} for session {cid} from connection {uuid}, which does not own it");
                // Dropped rather than answered: a caller acting on someone
                // else's session gets no confirmation that the session exists.
                return None;
            }
        }
    }

    match &command {
        InternalServiceRequest::GetAccountInformation { .. } => {
            get_account_information::handle(this, uuid, command).await
        }
        InternalServiceRequest::GetSessions { .. } => {
            get_sessions::handle(this, uuid, command).await
        }
        InternalServiceRequest::Connect { .. } => connect::handle(this, uuid, command).await,
        InternalServiceRequest::Register { .. } => register::handle(this, uuid, command).await,
        InternalServiceRequest::Message { .. } => message::handle(this, uuid, command).await,

        InternalServiceRequest::MediaOpen { .. } => media::handle_open(this, uuid, command).await,
        InternalServiceRequest::MediaSend { .. } => media::handle_send(this, uuid, command).await,
        InternalServiceRequest::MediaClose { .. } => media::handle_close(this, uuid, command).await,

        InternalServiceRequest::Disconnect { .. } => disconnect::handle(this, uuid, command).await,

        InternalServiceRequest::Deregister { .. } => deregister::handle(this, uuid, command).await,

        InternalServiceRequest::SendFile { .. } => file::upload::handle(this, uuid, command).await,

        InternalServiceRequest::RespondFileTransfer { .. } => {
            file::respond_file_transfer::handle(this, uuid, command).await
        }

        InternalServiceRequest::DownloadFile { .. } => {
            file::download::handle(this, uuid, command).await
        }

        InternalServiceRequest::DeleteVirtualFile { .. } => {
            file::delete_virtual_file::handle(this, uuid, command).await
        }

        InternalServiceRequest::PickFile { .. } => {
            file::pick_file::handle(this, uuid, command).await
        }

        InternalServiceRequest::ListRegisteredPeers { .. } => {
            peer::list_registered::handle(this, uuid, command).await
        }

        InternalServiceRequest::ListAllPeers { .. } => {
            peer::list_all::handle(this, uuid, command).await
        }

        InternalServiceRequest::PeerRegister { .. } => {
            peer::register::handle(this, uuid, command).await
        }

        InternalServiceRequest::PeerRegisterRespond { .. } => {
            peer::respond_register::handle(this, uuid, command).await
        }

        InternalServiceRequest::PeerConnect { .. } => {
            peer::connect::handle(this, uuid, command).await
        }

        InternalServiceRequest::PeerConnectAccept { .. } => {
            peer::accept::handle(this, uuid, command).await
        }

        InternalServiceRequest::PeerDisconnect { .. } => {
            peer::disconnect::handle(this, uuid, command).await
        }

        InternalServiceRequest::LocalDBGetKV { .. } => {
            local_db::get_kv::handle(this, uuid, command).await
        }

        InternalServiceRequest::LocalDBSetKV { .. } => {
            local_db::set_kv::handle(this, uuid, command).await
        }

        InternalServiceRequest::LocalDBDeleteKV { .. } => {
            local_db::delete_kv::handle(this, uuid, command).await
        }

        InternalServiceRequest::LocalDBGetAllKV { .. } => {
            local_db::get_all_kv::handle(this, uuid, command).await
        }

        InternalServiceRequest::LocalDBClearAllKV { .. } => {
            local_db::clear_all_kv::handle(this, uuid, command).await
        }

        InternalServiceRequest::GroupCreate { .. } => {
            group::create::handle(this, uuid, command).await
        }

        InternalServiceRequest::GroupLeave { .. } => {
            group::leave::handle(this, uuid, command).await
        }

        InternalServiceRequest::GroupEnd { .. } => group::end::handle(this, uuid, command).await,

        InternalServiceRequest::GroupMessage { .. } => {
            group::message::handle(this, uuid, command).await
        }

        InternalServiceRequest::GroupInvite { .. } => {
            group::invite::handle(this, uuid, command).await
        }

        InternalServiceRequest::GroupKick { .. } => group::kick::handle(this, uuid, command).await,

        InternalServiceRequest::GroupListGroupsFor { .. } => {
            group::group_list_groups::handle(this, uuid, command).await
        }

        InternalServiceRequest::GroupRespondRequest { .. } => {
            group::respond_request::handle(this, uuid, command).await
        }

        InternalServiceRequest::GroupRequestJoin { .. } => {
            group::request_join::handle(this, uuid, command).await
        }

        InternalServiceRequest::ConnectionManagement { .. } => {
            connection_management::handle(this, uuid, command).await
        }

        InternalServiceRequest::Batched {
            request_id,
            commands,
        } => {
            log::info!(target: "citadel", "[Batched] Received batched request with {} commands, request_id={}", commands.len(), request_id);
            // Execute all commands in parallel using FuturesOrdered to preserve order
            let mut futures: FuturesOrdered<
                Pin<Box<dyn std::future::Future<Output = Option<InternalServiceResponse>> + Send>>,
            > = FuturesOrdered::new();

            for cmd in commands.clone() {
                // The CONNECTION uuid, not the inner command's request_id.
                //
                // `handle_request`'s second parameter is the localhost
                // connection id everywhere else — it is what the session
                // ownership gate compares against, and what handlers use to
                // find this client in tx_to_localhost_clients. Passing a
                // request_id there meant every session-scoped sub-command in a
                // batch was silently refused (a random uuid can never equal
                // the real connection's), and a client that learned another
                // connection's uuid could have named it to pass the gate.
                //
                // Nothing is lost: the batch arm maps to `result.response` and
                // discards the returned uuid, and each handler takes its
                // response's request_id from the command itself.
                let fut = Box::pin(async move {
                    // Recursive call to handle each command
                    handle_request(this, uuid, cmd)
                        .await
                        .map(|result| result.response)
                })
                    as Pin<
                        Box<
                            dyn std::future::Future<Output = Option<InternalServiceResponse>>
                                + Send,
                        >,
                    >;
                futures.push_back(fut);
            }

            // Collect all results in order
            let results: Vec<InternalServiceResponse> =
                futures.filter_map(|r| async { r }).collect().await;

            log::info!(target: "citadel", "[Batched] Completed batched request, returning {} results, request_id={}", results.len(), request_id);
            Some(HandledRequestResult {
                response: InternalServiceResponse::BatchedResponse(BatchedResponseData {
                    cid: 0, // Batched requests are not tied to a single session
                    request_id: Some(*request_id),
                    results,
                }),
                uuid,
            })
        }
    }
}

pub(crate) fn spawn_group_channel_receiver(
    group_key: MessageGroupKey,
    implicated_cid: u64,
    uuid: Uuid,
    mut rx: GroupChannelRecvHalf,
    tcp_connection_map: Arc<
        parking_lot::RwLock<HashMap<Uuid, UnboundedSender<InternalServiceResponse>>>,
    >,
) {
    // Handler/Receiver for Group Channel Broadcasts that aren't handled in on_node_event_received in Kernel
    let group_channel_receiver = async move {
        while let Some(inbound_group_broadcast) = rx.next().await {
            // Gets UnboundedSender to the TCP client to forward Broadcasts
            match tcp_connection_map.read().get(&uuid) {
                Some(entry) => {
                    log::trace!(target:"citadel", "User {implicated_cid:?} Received Group Broadcast: {inbound_group_broadcast:?}");
                    let message = match inbound_group_broadcast {
                        GroupBroadcastPayload::Message { payload, sender } => {
                            Some(InternalServiceResponse::GroupMessageNotification(
                                GroupMessageNotification {
                                    cid: implicated_cid,
                                    peer_cid: sender,
                                    message: payload.into_buffer().into(),
                                    group_key,
                                    request_id: None,
                                },
                            ))
                        }
                        GroupBroadcastPayload::Event { payload } => match payload {
                            GroupBroadcast::RequestJoin { sender, key: _ } => {
                                Some(InternalServiceResponse::GroupJoinRequestNotification(
                                    GroupJoinRequestNotification {
                                        cid: implicated_cid,
                                        peer_cid: sender,
                                        group_key,
                                        request_id: None,
                                    },
                                ))
                            }
                            GroupBroadcast::MemberStateChanged { key: _, state } => {
                                Some(InternalServiceResponse::GroupMemberStateChangeNotification(
                                    GroupMemberStateChangeNotification {
                                        cid: implicated_cid,
                                        group_key,
                                        state,
                                        request_id: None,
                                    },
                                ))
                            }
                            GroupBroadcast::EndResponse { key, success } => {
                                Some(InternalServiceResponse::GroupEndNotification(
                                    GroupEndNotification {
                                        cid: implicated_cid,
                                        group_key: key,
                                        success,
                                        request_id: None,
                                    },
                                ))
                            }
                            GroupBroadcast::Disconnected { key } => {
                                Some(InternalServiceResponse::GroupDisconnectNotification(
                                    GroupDisconnectNotification {
                                        cid: implicated_cid,
                                        group_key: key,
                                        request_id: None,
                                    },
                                ))
                            }
                            GroupBroadcast::MessageResponse { key, success } => {
                                Some(InternalServiceResponse::GroupMessageResponse(
                                    GroupMessageResponse {
                                        cid: implicated_cid,
                                        group_key: key,
                                        success,
                                        request_id: None,
                                    },
                                ))
                            }
                            // GroupBroadcast::Create { .. } => {},
                            // GroupBroadcast::LeaveRoom { .. } => {},
                            // GroupBroadcast::End { .. } => {},
                            // GroupBroadcast::Add { .. } => {},
                            // GroupBroadcast::AddResponse { .. } => {},
                            // GroupBroadcast::AcceptMembership { .. } => {},
                            // GroupBroadcast::DeclineMembership { .. } => {},
                            // GroupBroadcast::AcceptMembershipResponse { .. } => {},
                            // GroupBroadcast::DeclineMembershipResponse { .. } => {},
                            // GroupBroadcast::Kick { .. } => {},
                            // GroupBroadcast::KickResponse { .. } => {},
                            // GroupBroadcast::ListGroupsFor { .. } => {},
                            // GroupBroadcast::ListResponse { .. } => {},
                            // GroupBroadcast::Invitation { .. } => {},
                            // GroupBroadcast::CreateResponse { .. } => {},
                            // GroupBroadcast::RequestJoinPending { .. } => {},
                            _ => None,
                        },
                    };

                    // Forward Group Broadcast to TCP Client if it was one of the handled broadcasts
                    if let Some(message) = message {
                        if let Err(err) = entry.send(message) {
                            info!(target: "citadel", "Group Channel Forward To TCP Client Failed: {err:?}");
                        }
                    }
                }
                None => {
                    info!(target:"citadel","Connection not found when Group Channel Broadcast Received");
                }
            }
        }
    };

    // Spawns the above Handler for Group Channel Broadcasts not handled in Node Events
    tokio::task::spawn(group_channel_receiver);
}

/// The seven keys ILM's messenger backend reads, each suffixed with `-{cid}`.
///
/// Kept here rather than imported so the gate does not depend on the connector
/// crate: this is a security boundary, and it should fail closed on a key it
/// does not recognise even if that crate changes shape. If ILM gains a key, a
/// refusal appears in the log naming the variant — which is how the original
/// evidence for this exemption was gathered in the first place.
const ILM_KEY_PREFIXES: [&str; 7] = [
    "inbound_messages-",
    "outbound_messages-",
    "last_acked-",
    "last_sent-",
    "next_unique_id-",
    "received_messages-",
    "last_received_from-",
];

/// Whether this request may name a session the connection does not own.
///
/// Extracted so the decision is testable on its own: it is one line at the call
/// site, and a control that widens it back to the whole variant has to fail
/// something. Previously the only tests covered the key predicate, so putting
/// `true` here broke nothing.
pub(crate) fn is_exempt_from_ownership_gate(command: &InternalServiceRequest) -> bool {
    match command {
        InternalServiceRequest::LocalDBGetKV { key, .. } => is_ilm_key(key.as_str()),
        _ => false,
    }
}

/// Whether this request needs the named session to be OWNED, not merely
/// unclaimed. See the gate for why an unmapped cid is otherwise let through.
pub(crate) fn requires_owned_session(command: &InternalServiceRequest) -> bool {
    matches!(
        command,
        InternalServiceRequest::LocalDBSetKV { .. }
            | InternalServiceRequest::LocalDBDeleteKV { .. }
            | InternalServiceRequest::LocalDBClearAllKV { .. }
            | InternalServiceRequest::LocalDBGetAllKV { .. }
    )
}

fn is_ilm_key(key: &str) -> bool {
    ILM_KEY_PREFIXES.iter().any(|prefix| {
        let Some(tail) = key.strip_prefix(prefix) else {
            return false;
        };
        // Non-empty checked first: `all()` on an empty tail is vacuously true,
        // so `"inbound_messages-"` with nothing after it rode the exemption.
        // The unit test below caught that in this very function.
        !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit())
    })
}

#[cfg(test)]
mod ownership_gate_tests {
    use super::{is_exempt_from_ownership_gate, is_ilm_key, requires_owned_session};
    use citadel_internal_service_types::InternalServiceRequest;
    use uuid::Uuid;

    fn get_kv(key: &str) -> InternalServiceRequest {
        InternalServiceRequest::LocalDBGetKV {
            request_id: Uuid::new_v4(),
            cid: 1,
            peer_cid: None,
            key: key.to_string(),
        }
    }

    #[test]
    fn only_ilm_reads_may_name_a_session_the_connection_does_not_own() {
        assert!(is_exempt_from_ownership_gate(&get_kv("last_sent-123")));
        // The whole variant used to be exempt, so any key rode through.
        assert!(!is_exempt_from_ownership_gate(&get_kv("credentials")));
    }

    #[test]
    fn no_other_request_is_exempt() {
        let write = InternalServiceRequest::LocalDBSetKV {
            request_id: Uuid::new_v4(),
            cid: 1,
            peer_cid: None,
            key: "last_sent-123".to_string(),
            value: vec![],
        };
        // An ILM-shaped KEY must not exempt a WRITE.
        assert!(!is_exempt_from_ownership_gate(&write));
        assert!(requires_owned_session(&write));
    }

    #[test]
    fn every_local_db_write_requires_an_owned_session() {
        let id = Uuid::new_v4();
        for command in [
            InternalServiceRequest::LocalDBSetKV {
                request_id: id, cid: 1, peer_cid: None, key: "k".into(), value: vec![],
            },
            InternalServiceRequest::LocalDBDeleteKV {
                request_id: id, cid: 1, peer_cid: None, key: "k".into(),
            },
            InternalServiceRequest::LocalDBClearAllKV { request_id: id, cid: 1, peer_cid: None },
            InternalServiceRequest::LocalDBGetAllKV { request_id: id, cid: 1, peer_cid: None },
        ] {
            assert!(
                requires_owned_session(&command),
                "an unmapped cid must not be enough for {command:?}",
            );
        }
    }

    #[test]
    fn recognises_every_key_ilm_actually_uses() {
        for key in [
            "inbound_messages-123",
            "outbound_messages-123",
            "last_acked-123",
            "last_sent-123",
            "next_unique_id-123",
            "received_messages-123",
            "last_received_from-123",
        ] {
            assert!(is_ilm_key(key), "{key} is a key ILM reads on the happy path");
        }
    }

    #[test]
    fn refuses_anything_else() {
        for key in [
            // The whole point: an arbitrary key used to ride the exemption.
            "credentials",
            "session-token",
            // A prefix match alone is not enough — the tail must be a cid.
            "last_sent-../credentials",
            "inbound_messages-abc",
            "inbound_messages-",
            // And a lookalike must not pass.
            "not_last_sent-123",
        ] {
            assert!(!is_ilm_key(key), "{key} must not ride the ILM exemption");
        }
    }
}
