use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, LocalDBSetKVFailure, LocalDBSetKVSuccess,
};
use citadel_sdk::backend_kv_store::BackendHandler;
use citadel_sdk::prelude::Ratchet;
use uuid::Uuid;

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::LocalDBSetKV {
        request_id,
        cid,
        peer_cid,
        key,
        value,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };

    let remote = super::generate_remote(this.remote(), cid, peer_cid).await;
    let response = backend_handler_set(&remote, cid, peer_cid, key, value, Some(request_id)).await;

    Some(HandledRequestResult { response, uuid })
}

// backend_handler_set
#[allow(clippy::too_many_arguments)]
async fn backend_handler_set<R: Ratchet>(
    remote: &impl BackendHandler<R>,
    cid: u64,
    peer_cid: Option<u64>,
    key: String,
    value: Vec<u8>,
    request_id: Option<Uuid>,
) -> InternalServiceResponse {
    match remote.set(&key, value).await {
        Ok(_) => InternalServiceResponse::LocalDBSetKVSuccess(LocalDBSetKVSuccess {
            cid,
            peer_cid,
            key,
            request_id,
        }),
        Err(err) => InternalServiceResponse::LocalDBSetKVFailure(LocalDBSetKVFailure {
            cid,
            peer_cid,
            message: err.into_string(),
            request_id,
        }),
    }
}
