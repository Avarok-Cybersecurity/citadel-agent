use crate::kernel::requests::{spawn_group_channel_receiver, HandledRequestResult};
use crate::kernel::{CitadelWorkspaceService, GroupConnection};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    GroupRespondRequestFailure, GroupRespondRequestSuccess, InternalServiceRequest,
    InternalServiceResponse,
};
use citadel_sdk::prelude::{
    GroupBroadcast, GroupBroadcastCommand, GroupChannelCreated, GroupEvent, NodeRequest,
    NodeResult, Ratchet,
};
use futures::StreamExt;
use std::sync::atomic::Ordering;
use uuid::Uuid;

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::GroupRespondRequest {
        cid,
        peer_cid,
        group_key,
        response,
        request_id,
        invitation,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };

    let group_request = if response {
        GroupBroadcast::AcceptMembership {
            target: if invitation { cid } else { peer_cid },
            key: group_key,
        }
    } else {
        GroupBroadcast::DeclineMembership {
            target: if invitation { cid } else { peer_cid },
            key: group_key,
        }
    };

    let request = NodeRequest::GroupBroadcastCommand(GroupBroadcastCommand {
        session_cid: cid,
        command: group_request,
    });

    // Extract the localhost-connection uuid inside the lock block, then drop
    // before await. A peer connection to the inviter is deliberately NOT
    // required: AcceptMembership is a client-to-server broadcast command (the
    // SERVER owns group membership; compare create.rs, which builds its remote
    // from the node remote for the same reason). Requiring a peer remote meant
    // every invitee whose P2P connection was ACCEPTED rather than initiated —
    // an acceptor-only connection carries no remote, and the group creator is
    // normally the one who dialled — could never answer any invitation, so
    // membership silently never formed.
    let remote_result = {
        let server_connection_map = this.server_connection_map.read();
        match server_connection_map.get(&cid) {
            Some(connection) => {
                let uuid = connection
                    .associated_localhost_connection
                    .load(Ordering::Relaxed);
                Ok((this.remote().clone(), uuid))
            }
            None => Err("Could Not Respond to Group Request - Connection not found".to_string()),
        }
    }; // Lock dropped here - BEFORE any await

    let response = match remote_result {
        Ok((remote, uuid)) => {
            match remote.send_callback_subscription(request).await {
                Ok(mut subscription) => {
                    let mut result = false;
                    if invitation {
                        while let Some(evt) = subscription.next().await {
                            match evt {
                                // When accepting an invite, we expect a GroupChannelCreated in response
                                NodeResult::GroupChannelCreated(GroupChannelCreated {
                                    ticket: _,
                                    channel,
                                    ..
                                }) => {
                                    let key = channel.key();
                                    let group_cid = channel.cid();
                                    let (tx, rx) = channel.split();
                                    if let Some(connection) =
                                        this.server_connection_map.write().get_mut(&cid)
                                    {
                                        connection.add_group_channel(
                                            key,
                                            GroupConnection {
                                                key,
                                                tx,
                                                cid: group_cid,
                                            },
                                        );

                                        spawn_group_channel_receiver(
                                            key,
                                            cid,
                                            uuid,
                                            rx,
                                            this.tx_to_localhost_clients.clone(),
                                        );

                                        result = true;
                                    } else {
                                        citadel_sdk::logging::error!(target: "citadel", "Connection {} not found in server_connection_map during group respond request", cid);
                                        result = false;
                                    }
                                    break;
                                }
                                NodeResult::GroupEvent(GroupEvent {
                                    session_cid: _,
                                    ticket: _,
                                    event:
                                        GroupBroadcast::AcceptMembershipResponse { key: _, success },
                                }) => {
                                    result = success;
                                    break;
                                }
                                NodeResult::GroupEvent(GroupEvent {
                                    session_cid: _,
                                    ticket: _,
                                    event:
                                        GroupBroadcast::DeclineMembershipResponse { key: _, success },
                                }) => {
                                    result = success;
                                    break;
                                }
                                _ => {}
                            };
                        }
                    } else {
                        // For now we return a Success response - we did, in fact, receive the KernelStreamSubscription
                        result = true;
                    }

                    match result {
                        true => InternalServiceResponse::GroupRespondRequestSuccess(
                            GroupRespondRequestSuccess {
                                cid,
                                group_key,
                                request_id: Some(request_id),
                            },
                        ),
                        false => InternalServiceResponse::GroupRespondRequestFailure(
                            GroupRespondRequestFailure {
                                cid,
                                message: "Group Invite Response Failed.".to_string(),
                                request_id: Some(request_id),
                            },
                        ),
                    }
                }
                Err(err) => InternalServiceResponse::GroupRespondRequestFailure(
                    GroupRespondRequestFailure {
                        cid,
                        message: err.to_string(),
                        request_id: Some(request_id),
                    },
                ),
            }
        }
        Err(message) => {
            InternalServiceResponse::GroupRespondRequestFailure(GroupRespondRequestFailure {
                cid,
                message,
                request_id: Some(request_id),
            })
        }
    };

    Some(HandledRequestResult { response, uuid })
}
