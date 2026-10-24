//! `AppsGetAgentSettings` and `AppsSetAgentSettings`: the member's own grants on this agent to
//! the apps it runs for them (kernel/apps/settings.rs), for the session's own windows only.

use crate::kernel::apps::settings::AgentAppSettings;
use crate::kernel::conversations::kv::HostKv;
use crate::kernel::ilm::HostIo;
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    AppsAgentSettings, AppsAgentSettingsFailure, InternalServiceRequest, InternalServiceResponse,
};
use citadel_sdk::prelude::Ratchet;
use std::sync::Arc;
use uuid::Uuid;

pub async fn handle<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let Some(cid) = request.session_cid() else {
        unreachable!("dispatched by variant")
    };
    let attached = this.attached(cid, uuid);
    let response = respond(&this.apps.settings, this.host_io(), attached, request).await;
    Some(HandledRequestResult { response, uuid })
}

/// The answer, for a caller that is or is not attached to the session the request names.
pub(crate) async fn respond(
    settings: &AgentAppSettings,
    io: Arc<dyn HostIo>,
    attached: bool,
    request: InternalServiceRequest,
) -> InternalServiceResponse {
    let (request_id, cid, set) = match request {
        InternalServiceRequest::AppsGetAgentSettings { request_id, cid } => (request_id, cid, None),
        InternalServiceRequest::AppsSetAgentSettings {
            request_id,
            cid,
            private_network,
        } => (request_id, cid, Some(private_network)),
        _ => unreachable!("dispatched by variant"),
    };
    let failure = |message: String| {
        InternalServiceResponse::AppsAgentSettingsFailure(AppsAgentSettingsFailure {
            cid,
            request_id: Some(request_id),
            message,
        })
    };
    if !attached {
        return failure("The session is not this connection's".to_string());
    }
    let store = HostKv::account(io, cid);
    if let Some(on) = set {
        if let Err(why) = settings.set_private_network(&store, cid, on).await {
            return failure(format!("The setting was not saved: {why}"));
        }
    }
    match settings.private_network(&store, cid).await {
        Ok(granted) => InternalServiceResponse::AppsAgentSettings(AppsAgentSettings {
            cid,
            request_id: Some(request_id),
            private_network: granted == Some(true),
        }),
        Err(why) => failure(format!("The setting could not be read: {why}")),
    }
}

#[cfg(test)]
#[path = "apps_settings_tests.rs"]
mod tests;
