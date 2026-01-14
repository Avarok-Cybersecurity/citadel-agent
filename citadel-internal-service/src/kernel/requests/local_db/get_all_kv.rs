use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, LocalDBGetAllKVFailure, LocalDBGetAllKVSuccess,
};
use citadel_sdk::backend_kv_store::BackendHandler;
use citadel_sdk::prelude::Ratchet;
use uuid::Uuid;

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::LocalDBGetAllKV {
        request_id,
        cid,
        peer_cid,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };

    let remote = super::generate_remote(this.remote(), cid, peer_cid).await;
    let response = backend_handler_get_all(&remote, cid, peer_cid, Some(request_id)).await;

    Some(HandledRequestResult { response, uuid })
}

// backend handler get_all
async fn backend_handler_get_all<R: Ratchet>(
    remote: &impl BackendHandler<R>,
    cid: u64,
    peer_cid: Option<u64>,
    request_id: Option<Uuid>,
) -> InternalServiceResponse {
    match remote.get_all().await {
        Ok(map) => InternalServiceResponse::LocalDBGetAllKVSuccess(LocalDBGetAllKVSuccess {
            cid,
            peer_cid,
            map,
            request_id,
        }),
        Err(err) => InternalServiceResponse::LocalDBGetAllKVFailure(LocalDBGetAllKVFailure {
            cid,
            peer_cid,
            message: err.into_string(),
            request_id,
        }),
    }
}
