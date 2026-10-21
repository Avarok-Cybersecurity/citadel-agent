//! `AppKv`: an app's `local` storage, in the account's own LocalDB (kernel/apps/kv).
//!
//! The namespace is built here from the request's app id and scope key and the SESSION's
//! username, never a name the request supplies, and every key is placed inside it.

use crate::kernel::apps::kv::Namespace;
use crate::kernel::conversations::kv::HostKv;
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    AppKvFailure, AppKvRefusal, AppKvResponse, InternalServiceRequest, InternalServiceResponse,
};
use citadel_sdk::prelude::Ratchet;
use uuid::Uuid;

pub async fn handle<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::AppKv {
        request_id,
        cid,
        namespace,
        op,
    } = request
    else {
        unreachable!("dispatched by variant")
    };
    let refuse = |refusal| {
        InternalServiceResponse::AppKvFailure(AppKvFailure {
            cid,
            request_id: Some(request_id),
            refusal,
        })
    };
    let username = {
        let sessions = this.server_connection_map.read();
        sessions
            .get(&cid)
            .filter(|conn| conn.subscribers.contains(uuid))
            .map(|conn| conn.username.clone())
    };
    let response = match username {
        None => refuse(AppKvRefusal::Session),
        Some(username) => {
            match Namespace::new(&namespace.app_id, &namespace.scope_key, &username) {
                Err(refusal) => refuse(refusal),
                Ok(ns) => {
                    let store = HostKv::account(this.host_io(), cid);
                    match this.apps.kv.run(&store, &ns, op).await {
                        Ok(result) => InternalServiceResponse::AppKvResponse(AppKvResponse {
                            cid,
                            request_id: Some(request_id),
                            result,
                        }),
                        Err(refusal) => refuse(refusal),
                    }
                }
            }
        }
    };
    Some(HandledRequestResult { response, uuid })
}
