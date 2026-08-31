use crate::kernel::requests::group::respond_wait::{
    await_invitation_outcome, InvitationOutcome, GROUP_RESPOND_WAIT,
};
use crate::kernel::requests::{spawn_group_channel_receiver, HandledRequestResult};
use crate::kernel::session_route::SessionRoute;
use crate::kernel::{CitadelWorkspaceService, GroupConnection};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    GroupRespondRequestFailure, GroupRespondRequestSuccess, InternalServiceRequest,
    InternalServiceResponse,
};
use citadel_sdk::logging::warn;
use citadel_sdk::prelude::{GroupBroadcast, GroupBroadcastCommand, NodeRequest, Ratchet};
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
            // The uuid is deliberately NOT captured here. It used to be, and
            // was handed to the spawned group receiver -- freezing the route at
            // the moment the request was answered. See kernel/session_route.rs.
            Some(_connection) => Ok(this.remote().clone()),
            None => Err("Could Not Respond to Group Request - Connection not found".to_string()),
        }
    }; // Lock dropped here - BEFORE any await

    let response = match remote_result {
        Ok(remote) => {
            match remote.send_callback_subscription(request).await {
                Ok(mut subscription) => {
                    let result = if invitation {
                        match await_invitation_outcome(&mut subscription, GROUP_RESPOND_WAIT).await
                        {
                            InvitationOutcome::ChannelCreated(channel) => {
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
                                        SessionRoute::new(
                                            connection.associated_localhost_connection.clone(),
                                            this.tx_to_localhost_clients.clone(),
                                        ),
                                        rx,
                                    );

                                    true
                                } else {
                                    citadel_sdk::logging::error!(target: "citadel", "Connection {} not found in server_connection_map during group respond request", cid);
                                    false
                                }
                            }
                            InvitationOutcome::MembershipAnswered(success) => success,
                            InvitationOutcome::Ended => false,
                            InvitationOutcome::TimedOut => {
                                warn!(target: "citadel", "Group respond for CID {cid} received no answer within {GROUP_RESPOND_WAIT:?}; the group owner may be offline");
                                false
                            }
                        }
                    } else {
                        // For now we return a Success response - we did, in fact, receive the KernelStreamSubscription
                        true
                    };

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
