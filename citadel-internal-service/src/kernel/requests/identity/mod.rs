//! The identity requests (kernel/identity): the lifecycle, discovery, and the two account
//! requests that use the identity, `RegisterIdentity` and `ConnectIdentity`.

pub(crate) mod connect;
mod post_connect;
pub(crate) mod register;

use crate::kernel::identity::{IdentityHost, Refused};
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    IdentityFailure, IdentityOutcome, IdentityRefusal, IdentitySuccess, InternalServiceRequest,
    InternalServiceResponse, ServerDiscoveryFailure, ServerDiscoverySuccess,
};
use citadel_sdk::prelude::Ratchet;
use std::sync::Arc;
use uuid::Uuid;
use zeroize::Zeroizing;

pub(crate) fn not_configured() -> Refused {
    (
        IdentityRefusal::NotConfigured,
        "this agent was started without an identity store".to_string(),
    )
}

pub(crate) fn host<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
) -> Result<Arc<IdentityHost>, Refused> {
    this.identity.clone().ok_or_else(not_configured)
}

fn secret(buffer: &citadel_internal_service_types::SecBuffer) -> Zeroizing<Vec<u8>> {
    Zeroizing::new(buffer.as_ref().to_vec())
}

fn framed(
    uuid: Uuid,
    request_id: Uuid,
    cid: u64,
    result: Result<IdentityOutcome, Refused>,
) -> Option<HandledRequestResult> {
    let response = match result {
        Ok(outcome) => InternalServiceResponse::IdentitySuccess(IdentitySuccess {
            cid,
            request_id: Some(request_id),
            outcome,
        }),
        Err((reason, message)) => InternalServiceResponse::IdentityFailure(IdentityFailure {
            cid,
            request_id: Some(request_id),
            reason,
            message,
        }),
    };
    Some(HandledRequestResult { response, uuid })
}

pub(crate) async fn lifecycle<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    command: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let request_id = *command.request_id()?;
    let cid = match &command {
        InternalServiceRequest::IdentityBinding { cid, .. } => *cid,
        _ => 0,
    };
    let result = match host(this) {
        Err(refused) => Err(refused),
        Ok(host) => match command {
            InternalServiceRequest::IdentityStatus { .. } => host.status().await,
            InternalServiceRequest::IdentityCreate {
                unlock_password, ..
            } => host.create(secret(&unlock_password)).await,
            InternalServiceRequest::IdentityUnlock {
                unlock_password, ..
            } => host.unlock(secret(&unlock_password)).await,
            InternalServiceRequest::IdentityLock { .. } => {
                host.unlocked.lock();
                Ok(IdentityOutcome::Locked)
            }
            InternalServiceRequest::IdentityExport {
                unlock_password, ..
            } => host.export(secret(&unlock_password)).await,
            InternalServiceRequest::IdentityImport {
                phrase,
                unlock_password,
                ..
            } => host.import(secret(&phrase), secret(&unlock_password)).await,
            InternalServiceRequest::IdentityBinding { cid, .. } => {
                connect::binding(this, cid).await
            }
            _ => unreachable!("dispatched by requests/mod.rs"),
        },
    };
    framed(uuid, request_id, cid, result)
}

pub(crate) async fn discover<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    command: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::ServerDiscovery {
        request_id,
        server_addr,
    } = command
    else {
        unreachable!("dispatched by requests/mod.rs")
    };
    let response = match register::discover(this, &server_addr).await {
        Ok(doc) => InternalServiceResponse::ServerDiscoverySuccess(ServerDiscoverySuccess {
            cid: 0,
            request_id: Some(request_id),
            canonical_domain: doc.canonical_domain.to_string(),
            citadel_wss: doc.citadel_wss,
            server_fp: citadel_identity::fingerprint(&doc.server_key),
            former_domains: doc.former_domains.iter().map(ToString::to_string).collect(),
            issued_at: doc.issued_at_s,
        }),
        Err(message) => InternalServiceResponse::ServerDiscoveryFailure(ServerDiscoveryFailure {
            cid: 0,
            request_id: Some(request_id),
            message,
        }),
    };
    Some(HandledRequestResult { response, uuid })
}
