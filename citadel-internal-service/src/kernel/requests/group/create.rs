use crate::kernel::requests::{spawn_group_channel_receiver, HandledRequestResult};
use crate::kernel::session_route::SessionRoute;
use crate::kernel::{CitadelWorkspaceService, GroupConnection};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    GroupCreateFailure, GroupCreateSuccess, InternalServiceRequest, InternalServiceResponse,
};
use citadel_sdk::prefabs::ClientServerRemote;
use citadel_sdk::prelude::{
    MessageGroupOptions, ProtocolRemoteTargetExt, Ratchet, UserIdentifier, VirtualTargetType,
};
use uuid::Uuid;

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::GroupCreate {
        cid,
        request_id,
        initial_users_to_invite,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };
    let options = MessageGroupOptions::default();
    let response = create_with(this, cid, request_id, initial_users_to_invite, options).await;
    Some(HandledRequestResult { response, uuid })
}

/// Create a group owned by the session `cid` with `options`, keep its channel and start
/// forwarding what it receives; the answer for `request_id`.
pub(super) async fn create_with<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    request_id: Uuid,
    initial_users_to_invite: Option<Vec<UserIdentifier>>,
    options: MessageGroupOptions,
) -> InternalServiceResponse {
    let remote = this.remote();

    // TODO: Add a simple targetted remote wrapper meant for transient/scoped requests
    let client_to_server_remote = ClientServerRemote::new(
        VirtualTargetType::LocalGroupServer { session_cid: cid },
        remote.clone(),
        Default::default(),
        None,
        None,
    );
    match client_to_server_remote
        .create_group_with_options(initial_users_to_invite, options)
        .await
    {
        Ok(group_channel) => {
            // Store the group connection in map
            let key = group_channel.key();
            let group_cid = group_channel.cid();
            let (tx, rx) = group_channel.split();
            match this.server_connection_map.write().get_mut(&cid) {
                Some(conn) => {
                    conn.add_group_channel(
                        key,
                        GroupConnection {
                            key,
                            cid: group_cid,
                            tx,
                        },
                    );

                    let route = SessionRoute::new(
                        conn.subscribers.clone(),
                        this.tx_to_localhost_clients.clone(),
                    );
                    let departed = conn.groups.departure_flag(&key);
                    spawn_group_channel_receiver(key, cid, route, departed, rx);

                    InternalServiceResponse::GroupCreateSuccess(GroupCreateSuccess {
                        cid,
                        group_key: key,
                        request_id: Some(request_id),
                    })
                }

                None => InternalServiceResponse::GroupCreateFailure(GroupCreateFailure {
                    cid,
                    message: format!("Server connection for {cid} not found"),
                    request_id: Some(request_id),
                }),
            }
        }

        Err(err) => InternalServiceResponse::GroupCreateFailure(GroupCreateFailure {
            cid,
            message: err.into_string(),
            request_id: Some(request_id),
        }),
    }
}
