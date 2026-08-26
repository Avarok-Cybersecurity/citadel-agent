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
    let exempt = matches!(command, InternalServiceRequest::LocalDBGetKV { .. });

    if let Some(cid) = command.session_cid().filter(|_| !exempt) {
        let owner = {
            let map = this.server_connection_map.read();
            map.get(&cid).map(|conn| {
                conn.associated_localhost_connection
                    .load(std::sync::atomic::Ordering::Relaxed)
            })
        };
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
                // Get the request_id from the inner command for the recursive call
                let cmd_uuid = cmd.request_id().copied().unwrap_or_else(Uuid::new_v4);
                let fut = Box::pin(async move {
                    // Recursive call to handle each command
                    handle_request(this, cmd_uuid, cmd)
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
