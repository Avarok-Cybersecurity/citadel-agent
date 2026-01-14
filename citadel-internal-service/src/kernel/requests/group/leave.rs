use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    GroupLeaveFailure, GroupLeaveSuccess, InternalServiceRequest, InternalServiceResponse,
};
use citadel_sdk::prelude::Ratchet;
use uuid::Uuid;

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::GroupLeave {
        cid,
        group_key,
        request_id,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };

    // Extract what we need inside the lock block, then drop before await
    let group_sender_result = {
        let server_connection_map = this.server_connection_map.read();
        match server_connection_map.get(&cid) {
            Some(connection) => match connection.groups.get(&group_key) {
                Some(group_connection) => Ok(group_connection.tx.clone()),
                None => Err("Could Not Leave Group - Group Connection not found".to_string()),
            },
            None => Err("Could Not Leave Group - Connection not found".to_string()),
        }
    }; // Lock dropped here - BEFORE any await

    let response = match group_sender_result {
        Ok(group_sender) => match group_sender.leave().await {
            Ok(_) => InternalServiceResponse::GroupLeaveSuccess(GroupLeaveSuccess {
                cid,
                group_key,
                request_id: Some(request_id),
            }),
            Err(err) => InternalServiceResponse::GroupLeaveFailure(GroupLeaveFailure {
                cid,
                message: err.into_string(),
                request_id: Some(request_id),
            }),
        },
        Err(message) => InternalServiceResponse::GroupLeaveFailure(GroupLeaveFailure {
            cid,
            message,
            request_id: Some(request_id),
        }),
    };

    Some(HandledRequestResult { response, uuid })
}
