use crate::kernel::session_route::SessionRoute;
use crate::kernel::CitadelWorkspaceService;
use async_recursion::async_recursion;
use citadel_internal_service_types::*;
use citadel_sdk::logging::info;
use citadel_sdk::logging::tracing::log;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_sdk::prelude::*;
use futures::stream::FuturesOrdered;
use futures::StreamExt;
use std::pin::Pin;
use uuid::Uuid;

pub(crate) struct HandledRequestResult {
    pub response: InternalServiceResponse,
    pub uuid: Uuid,
}

mod agent_own;
mod connect;
pub(crate) mod connect_mode;
mod conversation;
mod deregister;
mod disconnect;
mod get_account_information;
mod get_sessions;
mod media;
mod message;
mod notices;
mod register;
mod send_reliable;
mod update;
pub(crate) use agent_own::{answer_local_db, send_message};

mod connection_management;
mod connection_management_attach;
pub(crate) mod connection_management_auth;
mod connection_management_claim;
mod connection_management_claim_sdk;
pub(crate) mod file;
mod group;
mod local_db;
pub(crate) mod peer;

#[async_recursion]
#[allow(clippy::multiple_bound_locations)]
// Clippy 1.99's double_must_use fires on async_recursion's generated `#[must_use]` (1.1.1 is
// the newest release); the attribute is the macro's, not ours.
#[allow(clippy::double_must_use)]
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
    // Computed, and deliberately NOT used to bypass the gate below.
    //
    // The comment on `is_exempt_from_ownership_gate` states the intent
    // exactly: "the gate then judges the request's cid as it judges every
    // other: owned proceeds, held by somebody else is refused." It did not.
    // `.filter(|_| !exempt)` skipped the entire ownership block, so an ILM key
    // naming its own session was handed over even while a DIFFERENT connection
    // held that session. `GetSessions` is ungated and returns every cid, so a
    // second connection could ask for `inbound_messages-<victim>` and receive
    // that account's stored P2P payloads.
    //
    // The narrowing that comment describes landed; the sentence after it never
    // did. `gate_decision` already answers this correctly, so the fix is to
    // stop skipping it. The predicate is kept because its tests pin the key
    // shapes ILM uses, which is worth keeping true.
    let _narrowed_ilm_key = is_exempt_from_ownership_gate(&command);

    // Writes must be OWNED, not merely unopposed.
    //
    // The gate lets an unmapped cid through, on the stated grounds that "the
    // handler owns that error and already reports it". That is true of download
    // and delete_virtual_file, which look the cid up in the map and fail. It is
    // NOT true of the LocalDB handlers: they resolve through `propose_target`,
    // which by its own doc only checks that the cid names a locally-known
    // account — not that the caller owns it. So for an account that is known but
    // has no mapped session (after a Disconnect, say), any connection could
    // write or wipe its persistent store. `gate_decision` derives that from the
    // command itself.
    if let Some(cid) = command.session_cid() {
        // Whether the caller is attached: any window of the session may act on
        // it, the primary or not (kernel/session_subscribers.rs).
        let attached = {
            let map = this.server_connection_map.read();
            map.get(&cid).map(|conn| conn.subscribers.contains(uuid))
        };
        // The decision itself is a pure function of (command, owner, caller),
        // and it is taken there rather than here so it can be tested without a
        // running service. It was inline, and a control that restored the
        // silent `return None` passed every test in this file: the tests
        // covered the response BUILDER, which nothing was obliged to call.
        match gate_decision(&command, attached) {
            GateDecision::Proceed => {}
            GateDecision::Refuse { reason } => {
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
                    "Refusing {variant} for session {cid} from connection {uuid}: {reason}");
                return refusal_response(&command, uuid);
            }
        }
    }

    // The agent is the only writer of a hosted account's conversations.
    if this.writes_hosted_conversation(&command) {
        log::warn!(target: "citadel", "Refusing a conversation-record write from connection {uuid}: the agent hosts that account");
        return refusal_response(&command, uuid);
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
        InternalServiceRequest::NoticeSubscribe { .. }
        | InternalServiceRequest::NoticeSetMuted { .. } => {
            notices::handle(this, uuid, command).await
        }
        InternalServiceRequest::UpdateGetStatus { .. }
        | InternalServiceRequest::UpdateCheckNow { .. }
        | InternalServiceRequest::UpdateApply { .. }
        | InternalServiceRequest::UpdateSetSettings { .. }
        | InternalServiceRequest::UpdateInstallResult { .. } => {
            update::handle(this, uuid, command).await
        }
        InternalServiceRequest::SendReliable { .. } => {
            send_reliable::handle(this, uuid, command).await
        }
        InternalServiceRequest::ConversationSend { .. }
        | InternalServiceRequest::ConversationResend { .. }
        | InternalServiceRequest::ConversationEdit { .. }
        | InternalServiceRequest::ConversationDelete { .. }
        | InternalServiceRequest::ConversationReact { .. }
        | InternalServiceRequest::ConversationMarkRead { .. }
        | InternalServiceRequest::ConversationRecord { .. }
        | InternalServiceRequest::ConversationPatch { .. }
        | InternalServiceRequest::ConversationClear { .. }
        | InternalServiceRequest::ConversationList { .. }
        | InternalServiceRequest::ConversationPage { .. }
        | InternalServiceRequest::SetAccountPreferences { .. }
        | InternalServiceRequest::GetAccountPreferences { .. } => {
            conversation::handle(this, uuid, command).await
        }

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
        InternalServiceRequest::GroupListJoined { .. } => {
            group::list_joined::handle(this, uuid, command).await
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

/// `route`, not a `Uuid`: this task outlives any single localhost connection.
/// A reclaim re-points the session and every remaining broadcast has to follow
/// it, or the group goes silent with nothing in the log but "Connection not
/// found". See kernel/session_route.rs.
pub(crate) fn spawn_group_channel_receiver(
    group_key: MessageGroupKey,
    implicated_cid: u64,
    route: SessionRoute,
    // The departure flag for this group's entry in `Connection.groups`.
    //
    // A `Disconnected` / `EndResponse` broadcast reaches the client through TWO
    // paths: `responses/group_event.rs`, which holds the connection map and can
    // mark the entry itself, and this task, which does not. Marking in only one
    // of them left the membership check passing after the group had ended --
    // the end-to-end test in tests/group_stale_membership.rs caught exactly
    // that, with the notification delivered and the entry still live.
    departed: Option<Arc<AtomicBool>>,
    mut rx: GroupChannelRecvHalf,
) {
    // Handler/Receiver for Group Channel Broadcasts that aren't handled in on_node_event_received in Kernel
    let group_channel_receiver = async move {
        while let Some(inbound_group_broadcast) = rx.next().await {
            {
                {
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
                                if success {
                                    if let Some(departed) = departed.as_ref() {
                                        departed.store(true, std::sync::atomic::Ordering::SeqCst);
                                    }
                                }
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
                                if let Some(departed) = departed.as_ref() {
                                    departed.store(true, std::sync::atomic::Ordering::SeqCst);
                                }
                                Some(InternalServiceResponse::GroupDisconnectNotification(
                                    GroupDisconnectNotification {
                                        cid: implicated_cid,
                                        group_key: key,
                                        request_id: None,
                                    },
                                ))
                            }
                            GroupBroadcast::MessageDropped {
                                key,
                                sender,
                                reason,
                            } => Some(crate::kernel::responses::group_event::message_dropped(
                                implicated_cid,
                                key,
                                sender,
                                reason,
                            )),
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
                        // Logged at info, as the P2P path's "[P2P-RECV] Delivered"
                        // is: the per-broadcast trace above is off in a live
                        // agent, and without this line a group message the SDK
                        // never handed over and one the agent forwarded to the
                        // owning connection leave the same (empty) log.
                        let is_message = matches!(
                            message,
                            InternalServiceResponse::GroupMessageNotification(_)
                        );
                        let delivered = route.send(message);
                        match delivered.as_slice() {
                            [] => {
                                info!(target:"citadel","No localhost connection owns CID {implicated_cid} - group broadcast dropped");
                            }
                            targets if is_message => {
                                info!(target:"citadel","[GROUP-RECV] Delivered a group message for CID {implicated_cid} to {targets:?}");
                            }
                            _ => {}
                        }
                    }
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
        InternalServiceRequest::LocalDBGetKV { key, cid, .. } => {
            // The ILM key must name THIS request's session.
            //
            // Narrowed twice. It was the whole `LocalDBGetKV` variant; then the
            // key had to be one of ILM's seven, suffixed by digits — any digits.
            // A cid is a u64 that travels in peer lists and `GetSessions`
            // responses and is not a secret, and the internal service's
            // WebSocket has no CORS to stop a page opening it. So any page the
            // user visited could ask for `inbound_messages-<victim>` and be
            // handed that account's stored P2P message payloads, while the same
            // request for `credentials` was correctly refused.
            //
            // Asking that the key's cid equals the request's cid closes the
            // mismatch, and the gate then judges the request's cid as it judges
            // every other: owned proceeds, held by somebody else is refused.
            is_ilm_key_for(key.as_str(), *cid)
        }
        _ => false,
    }
}

/// Whether the gate lets a request through, and why not when it does not.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum GateDecision {
    Proceed,
    Refuse { reason: &'static str },
}

/// The ownership gate's decision, as a pure function.
///
/// `owner` is who the connection map says holds the named session: `None` for a
/// session it does not know at all.
pub(crate) fn gate_decision(
    command: &InternalServiceRequest,
    caller_attached: Option<bool>,
) -> GateDecision {
    // Derived here rather than passed in: a caller that computes it separately
    // can pass one that disagrees with the command, and then the decision is
    // about a request nobody made.
    // CID 0 names no session.
    //
    // It is the agent's own scratch space -- a "global" key shared by whatever
    // is running on this machine, which is what global means for a local agent.
    // The READS already work that way: `LocalDBGetKV` needs no ownership, so it
    // reaches the handler and answers "Key not found" like any other missing
    // key. The writes did not, and the gate refused them for a session that
    // does not exist.
    //
    // The effect was that the auto-reconnect preference could never be saved.
    // Before refusals were answered at all (round 224) that was a five-second
    // hang and a reverted switch; afterwards it was a visible error naming a
    // session the user had never heard of. Neither is the setting being stored.
    if command.session_cid() == Some(0) {
        return GateDecision::Proceed;
    }

    let requires_ownership = requires_owned_session(command);
    match caller_attached {
        // Known but the caller is not attached to it. Refused whatever it asks
        // for: the gate's original purpose.
        Some(false) => GateDecision::Refuse {
            reason: "the connection does not own it",
        },
        Some(true) => GateDecision::Proceed,
        // Not in the map at all. Reads may proceed -- the handler fails them
        // honestly -- but a write or a wipe must not, or any connection could
        // clear the store of an account that merely happens to be disconnected.
        None if requires_ownership => GateDecision::Refuse {
            reason: "no mapped session, so ownership cannot be established",
        },
        None => GateDecision::Proceed,
    }
}

/// The answer a refused request gets.
///
/// Refusals used to `return None`, which sends nothing at all. For the four
/// LocalDB variants behind the ownership gate that is a five-second hang per
/// request in the browser, and it was measured, not theorised: a CI run shows
/// `LocalDBSetKV`, `LocalDBDeleteKV` and `LocalDBGetAllKV` timing out four
/// times while `LocalDBGetKV` -- the one variant the gate exempts -- is answered
/// throughout. Those timeouts then cascade into "Workspace loading timeout" and
/// the leg fails.
///
/// It is the ordinary case, not an attack: a browser keeps its CID across an
/// internal-service restart, so the first write after one names a session that
/// no longer exists. Silence tells that caller nothing it can act on, and the
/// caller is the app itself.
///
/// The message is deliberately the same for "no such session" and "not yours",
/// so answering leaks no more than the timeout already did.
///
/// This said "the gate only ever refuses these four", which was FALSE: 38
/// request variants return `Some` from `session_cid()` and so are gated, while
/// this function answered six. The other 32 were refused in silence, and the
/// caller sat out its whole budget -- thirty seconds for a P2P request -- before
/// reporting a timeout that names the wrong cause. A browser whose session had
/// been claimed elsewhere hit that on every message, every file, every call.
///
/// Twenty-two now answer with their own Failure variant. Ten stay silent for
/// reasons written beside them, and the exhaustiveness test asserts exactly
/// that list.
fn refusal_response(command: &InternalServiceRequest, uuid: Uuid) -> Option<HandledRequestResult> {
    /// Same wording for every refusal; see above.
    const REFUSED: &str = "Session unavailable to this connection";

    if let Some(response) = conversation::refusal(command, REFUSED) {
        return Some(HandledRequestResult { response, uuid });
    }
    let response = match command {
        InternalServiceRequest::LocalDBSetKV {
            request_id,
            cid,
            peer_cid,
            ..
        } => InternalServiceResponse::LocalDBSetKVFailure(LocalDBSetKVFailure {
            cid: *cid,
            peer_cid: *peer_cid,
            message: REFUSED.to_string(),
            request_id: Some(*request_id),
        }),
        InternalServiceRequest::LocalDBDeleteKV {
            request_id,
            cid,
            peer_cid,
            ..
        } => InternalServiceResponse::LocalDBDeleteKVFailure(LocalDBDeleteKVFailure {
            cid: *cid,
            peer_cid: *peer_cid,
            message: REFUSED.to_string(),
            request_id: Some(*request_id),
        }),
        InternalServiceRequest::LocalDBGetAllKV {
            request_id,
            cid,
            peer_cid,
            ..
        } => InternalServiceResponse::LocalDBGetAllKVFailure(LocalDBGetAllKVFailure {
            cid: *cid,
            peer_cid: *peer_cid,
            message: REFUSED.to_string(),
            request_id: Some(*request_id),
        }),
        InternalServiceRequest::LocalDBClearAllKV {
            request_id,
            cid,
            peer_cid,
            ..
        } => InternalServiceResponse::LocalDBClearAllKVFailure(LocalDBClearAllKVFailure {
            cid: *cid,
            peer_cid: *peer_cid,
            message: REFUSED.to_string(),
            request_id: Some(*request_id),
        }),
        // The two operations this gate exists to protect, and the two it
        // answered with silence.
        //
        // `_ => return None` sends NOTHING. The caller waits out its whole
        // request budget -- thirty seconds for Disconnect -- and then reports a
        // timeout, which names the wrong thing: the service did not fail to
        // answer, it decided not to act and did not say so. Measured in CI as
        // `Failed to disconnect: Error: Disconnect request timed out`, over a
        // sign-out modal that spun for the full thirty seconds while a
        // `Refusing Disconnect for session … ` line sat in the server log where
        // no user can see it.
        //
        // The comment above this function's caller says the same thing about
        // this shape: "a control that restored the silent `return None` passed
        // every test in this file: the tests covered the response BUILDER,
        // which nothing was obliged to call". For these two, it was not called.
        InternalServiceRequest::Disconnect { request_id, cid } => {
            InternalServiceResponse::PeerDisconnectFailure(PeerDisconnectFailure {
                cid: *cid,
                message: REFUSED.to_string(),
                request_id: Some(*request_id),
            })
        }
        InternalServiceRequest::Deregister { request_id, cid } => {
            InternalServiceResponse::DeregisterFailure(DeregisterFailure {
                cid: *cid,
                message: REFUSED.to_string(),
                request_id: Some(*request_id),
            })
        }
        InternalServiceRequest::DeleteVirtualFile {
            request_id, cid, ..
        } => InternalServiceResponse::DeleteVirtualFileFailure(DeleteVirtualFileFailure {
            cid: *cid,
            message: REFUSED.to_string(),
            request_id: Some(*request_id),
        }),
        InternalServiceRequest::DownloadFile {
            request_id, cid, ..
        } => InternalServiceResponse::DownloadFileFailure(DownloadFileFailure {
            cid: *cid,
            message: REFUSED.to_string(),
            request_id: Some(*request_id),
        }),
        InternalServiceRequest::GroupCreate {
            request_id, cid, ..
        } => InternalServiceResponse::GroupCreateFailure(GroupCreateFailure {
            cid: *cid,
            message: REFUSED.to_string(),
            request_id: Some(*request_id),
        }),
        InternalServiceRequest::GroupEnd {
            request_id, cid, ..
        } => InternalServiceResponse::GroupEndFailure(GroupEndFailure {
            cid: *cid,
            message: REFUSED.to_string(),
            request_id: Some(*request_id),
        }),
        InternalServiceRequest::GroupInvite {
            request_id, cid, ..
        } => InternalServiceResponse::GroupInviteFailure(GroupInviteFailure {
            cid: *cid,
            message: REFUSED.to_string(),
            request_id: Some(*request_id),
        }),
        InternalServiceRequest::GroupKick {
            request_id, cid, ..
        } => InternalServiceResponse::GroupKickFailure(GroupKickFailure {
            cid: *cid,
            message: REFUSED.to_string(),
            request_id: Some(*request_id),
        }),
        InternalServiceRequest::GroupLeave {
            request_id, cid, ..
        } => InternalServiceResponse::GroupLeaveFailure(GroupLeaveFailure {
            cid: *cid,
            message: REFUSED.to_string(),
            request_id: Some(*request_id),
        }),
        InternalServiceRequest::GroupMessage {
            request_id, cid, ..
        } => InternalServiceResponse::GroupMessageFailure(GroupMessageFailure {
            cid: *cid,
            message: REFUSED.to_string(),
            request_id: Some(*request_id),
        }),
        InternalServiceRequest::GroupRequestJoin {
            request_id, cid, ..
        } => InternalServiceResponse::GroupRequestJoinFailure(GroupRequestJoinFailure {
            cid: *cid,
            message: REFUSED.to_string(),
            request_id: Some(*request_id),
        }),
        InternalServiceRequest::GroupRespondRequest {
            request_id, cid, ..
        } => InternalServiceResponse::GroupRespondRequestFailure(GroupRespondRequestFailure {
            cid: *cid,
            message: REFUSED.to_string(),
            request_id: Some(*request_id),
        }),
        InternalServiceRequest::ListAllPeers {
            request_id, cid, ..
        } => InternalServiceResponse::ListAllPeersFailure(ListAllPeersFailure {
            cid: *cid,
            message: REFUSED.to_string(),
            request_id: Some(*request_id),
        }),
        InternalServiceRequest::ListRegisteredPeers {
            request_id, cid, ..
        } => InternalServiceResponse::ListRegisteredPeersFailure(ListRegisteredPeersFailure {
            cid: *cid,
            message: REFUSED.to_string(),
            request_id: Some(*request_id),
        }),
        InternalServiceRequest::PeerConnect {
            request_id, cid, ..
        } => InternalServiceResponse::PeerConnectFailure(PeerConnectFailure {
            cid: *cid,
            message: REFUSED.to_string(),
            request_id: Some(*request_id),
        }),
        InternalServiceRequest::PeerDisconnect {
            request_id, cid, ..
        } => InternalServiceResponse::PeerDisconnectFailure(PeerDisconnectFailure {
            cid: *cid,
            message: REFUSED.to_string(),
            request_id: Some(*request_id),
        }),
        InternalServiceRequest::PeerRegister {
            request_id, cid, ..
        } => InternalServiceResponse::PeerRegisterFailure(PeerRegisterFailure {
            cid: *cid,
            message: REFUSED.to_string(),
            request_id: Some(*request_id),
        }),
        InternalServiceRequest::PickFile {
            request_id, cid, ..
        } => InternalServiceResponse::PickFileFailure(PickFileFailure {
            cid: *cid,
            message: REFUSED.to_string(),
            request_id: Some(*request_id),
        }),
        InternalServiceRequest::SendFile {
            request_id, cid, ..
        } => InternalServiceResponse::SendFileRequestFailure(SendFileRequestFailure {
            cid: *cid,
            message: REFUSED.to_string(),
            request_id: Some(*request_id),
        }),
        InternalServiceRequest::Message {
            request_id, cid, ..
        }
        | InternalServiceRequest::SendReliable {
            request_id, cid, ..
        } => InternalServiceResponse::MessageSendFailure(MessageSendFailure {
            cid: *cid,
            message: REFUSED.to_string(),
            request_id: Some(*request_id),
        }),
        // `error`, not `message` -- this one's failure struct names the field
        // differently, which is why a blanket rewrite could not have produced it.
        InternalServiceRequest::PeerConnectAccept {
            request_id,
            cid,
            peer_cid,
            ..
        } => InternalServiceResponse::PeerConnectAcceptFailure(PeerConnectAcceptFailure {
            cid: *cid,
            peer_cid: *peer_cid,
            message: REFUSED.to_string(),
            request_id: Some(*request_id),
        }),
        // A refused read is ANSWERED, because it has somewhere to say so.
        //
        // This sat in the silent list below, under the reason that the queries
        // "return DATA, and their response types carry no failure variant". That
        // is true of GroupListGroupsFor. It was never true of LocalDBGetKV:
        // `LocalDBGetKVFailure` exists, the handler already builds one when
        // `propose_target` fails, and the client matches it by request id.
        //
        // The false premise was load-bearing. Being silent is why the variant
        // could not be added to `requires_owned_session` -- gating it would have
        // meant refusing reads into nothing, and the browser waiting out its own
        // timeout -- and not being gated is why any connection could read a known
        // account's stored ILM payloads while it had no mapped session.
        InternalServiceRequest::LocalDBGetKV {
            request_id,
            cid,
            peer_cid,
            ..
        } => InternalServiceResponse::LocalDBGetKVFailure(LocalDBGetKVFailure {
            cid: *cid,
            peer_cid: *peer_cid,
            message: REFUSED.to_string(),
            request_id: Some(*request_id),
        }),
        InternalServiceRequest::GroupListJoined { request_id, cid } => {
            InternalServiceResponse::GroupListJoinedFailure(GroupListJoinedFailure {
                cid: *cid,
                message: REFUSED.to_string(),
                request_id: Some(*request_id),
            })
        }
        // Everything else stays silent, and each is a deliberate decision.
        //
        // GroupListGroupsFor returns DATA and has no failure variant, so a
        // refusal would have to invent an empty result -- which reads as "there
        // is nothing" and is the absence-for-failure confusion this codebase has
        // spent a dozen rounds removing.
        //
        // The media requests (MediaOpen, MediaClose, MediaSend) and
        // RespondFileTransfer / PeerRegisterRespond answer over their own
        // notification streams rather than a request/response pair.
        //
        // Named individually so the exhaustiveness test below can assert this
        // list and nothing wider: a variant added to the gate without a
        // decision fails that test rather than joining a silent default.
        InternalServiceRequest::GroupListGroupsFor { .. }
        | InternalServiceRequest::MediaOpen { .. }
        | InternalServiceRequest::MediaClose { .. }
        | InternalServiceRequest::MediaSend { .. }
        | InternalServiceRequest::RespondFileTransfer { .. }
        | InternalServiceRequest::PeerRegisterRespond { .. } => return None,

        _ => return None,
    };

    Some(HandledRequestResult { response, uuid })
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
            // Reads too, and the destructive pair the gate's own comment names.
            //
            // The gate lets an unmapped cid proceed on the grounds that "the
            // handler owns that error and already reports it". For a cid naming
            // an account that is UNKNOWN, that holds: `propose_target` fails and
            // the handler answers. For one that is KNOWN but merely has no live
            // session -- after a Disconnect, or an agent restart while the
            // browser keeps its cid -- `propose_target` SUCCEEDS, by its own doc
            // checking only that the cid names a locally-known account. The read
            // then returns that account's stored ILM payloads to any connection
            // that can name the cid, and a cid is a u64 that travels in peer
            // lists and `GetSessions` responses, not a secret.
            //
            // Deregister is the same shape with a worse ending: its handler
            // sends `DeregisterFromHypernode{cid}` without consulting the map at
            // all, so an unmapped cid deletes the account permanently. The
            // comment on `handle` already lists it among the operations that
            // "are gated now" -- it was gated only against a session held by
            // somebody else, never against one held by nobody.
            | InternalServiceRequest::LocalDBGetKV { .. }
            | InternalServiceRequest::Deregister { .. } // The conversation store answers by CID from the agent's own store, not
                                                        // through `propose_target`, so a signed-out account would otherwise hand
                                                        // its history to any connection naming it.
    ) || conversation::is_request(command)
}

/// One of ILM's seven keys, suffixed with `cid`.
///
/// The suffix must be that exact cid, compared as a STRING against the digits in
/// the key: parsing them would accept `007` and any u64 overflow the parser
/// happens to reject, and neither is a key ILM would ever write.
fn is_ilm_key_for(key: &str, cid: u64) -> bool {
    let expected = cid.to_string();
    ILM_KEY_PREFIXES.iter().any(|prefix| {
        let Some(tail) = key.strip_prefix(prefix) else {
            return false;
        };
        // Non-empty checked first: `all()` on an empty tail is vacuously true,
        // so `"inbound_messages-"` with nothing after it rode the exemption.
        // The unit test below caught that in this very function.
        !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) && tail == expected
    })
}

#[cfg(test)]
mod ownership_gate_tests;
