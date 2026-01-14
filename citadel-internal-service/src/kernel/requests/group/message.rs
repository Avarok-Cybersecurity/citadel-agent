use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    GroupMessageFailure, GroupMessageSuccess, InternalServiceRequest, InternalServiceResponse,
};
use citadel_sdk::prelude::Ratchet;
use uuid::Uuid;

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::GroupMessage {
        cid,
        message,
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
                None => Err("Could Not Message Group - Group Connection not found".to_string()),
            },
            None => Err("Could Not Message Group - Connection not found".to_string()),
        }
    }; // Lock dropped here - BEFORE any await

    let response = match group_sender_result {
        Ok(group_sender) => match group_sender.send_message(message.into()).await {
            Ok(_) => InternalServiceResponse::GroupMessageSuccess(GroupMessageSuccess {
                cid,
                group_key,
                request_id: Some(request_id),
            }),
            Err(err) => InternalServiceResponse::GroupMessageFailure(GroupMessageFailure {
                cid,
                message: err.to_string(),
                request_id: Some(request_id),
            }),
        },
        Err(msg) => InternalServiceResponse::GroupMessageFailure(GroupMessageFailure {
            cid,
            message: msg,
            request_id: Some(request_id),
        }),
    };

    Some(HandledRequestResult { response, uuid })
}
