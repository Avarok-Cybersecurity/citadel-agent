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

    // An unknown CID is a normal client input — the dev backend drops accounts
    // on restart while browsers keep their CIDs — and it used to panic this
    // task, so the request was never answered at all.
    let remote = match super::generate_remote(this.remote(), cid, peer_cid).await {
        Ok(remote) => remote,
        Err(err) => {
            let response = InternalServiceResponse::LocalDBSetKVFailure(LocalDBSetKVFailure {
                cid,
                peer_cid,
                message: err.into_string(),
                request_id: Some(request_id),
            });
            return Some(HandledRequestResult { response, uuid });
        }
    };
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
