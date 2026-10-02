//! The updater's requests (kernel/updates): status, check now, install now, the auto-install
//! setting, and the menu-bar app's report of an install it could not complete.

use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use crate::updater::engine::{Engine, AUTO_INSTALL_UNSET};
use crate::updater::policy::Trigger;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, NoticeFailure, UpdateStatus,
};
use citadel_sdk::logging::warn;
use citadel_sdk::prelude::Ratchet;
use std::sync::Arc;
use uuid::Uuid;

pub async fn handle<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let request_id = *request.request_id()?;
    let answer = |response| Some(HandledRequestResult { response, uuid });
    let Some(engine) = this.updates.engine.get().cloned() else {
        return answer(not_running(this, request_id));
    };
    let status = |error: Option<String>| {
        let mut status = engine.status(Some(request_id));
        if error.is_some() {
            status.last_error = error;
        }
        InternalServiceResponse::UpdateStatus(status)
    };
    match request {
        InternalServiceRequest::UpdateGetStatus { .. } => answer(status(None)),
        InternalServiceRequest::UpdateCheckNow { .. } => {
            engine.check().await;
            reconsider(&engine);
            answer(status(None))
        }
        InternalServiceRequest::UpdateApply { .. } => {
            answer(status(engine.consider(Trigger::UserAsked).await.err()))
        }
        InternalServiceRequest::UpdateSetSettings { auto_install, .. } => {
            let saved = engine.set_auto_install(auto_install).await;
            if saved.is_ok() {
                reconsider(&engine);
            }
            answer(status(
                saved
                    .err()
                    .map(|e| format!("The setting was not saved: {e}")),
            ))
        }
        InternalServiceRequest::UpdateInstallResult {
            token,
            version,
            error,
            ..
        } => {
            if !this.notices.admits(&token) {
                warn!(target: "citadel", "[UPDATE] connection {uuid} refused an install report");
                return answer(InternalServiceResponse::NoticeFailure(NoticeFailure {
                    cid: 0,
                    message: "The notice plane is not open to this connection".to_string(),
                    request_id: Some(request_id),
                }));
            }
            if let Some(why) = error {
                engine.install_failed(&version, &why);
            }
            answer(status(None))
        }
        _ => None,
    }
}

/// Reconsider installing, off the request's path: it may restart the agent.
fn reconsider(engine: &Arc<Engine>) {
    let engine = engine.clone();
    drop(tokio::task::spawn(async move {
        if let Err(e) = engine.consider(Trigger::Idle).await {
            warn!(target: "citadel::update", "{e}");
        }
    }));
}

fn not_running<T, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    request_id: Uuid,
) -> InternalServiceResponse {
    let current = this
        .updates
        .config
        .as_ref()
        .map(|c| c.current_version.clone())
        .unwrap_or_default();
    InternalServiceResponse::UpdateStatus(UpdateStatus {
        cid: 0,
        current,
        available: None,
        auto_install: AUTO_INSTALL_UNSET,
        last_checked: None,
        last_error: Some("The updater is not running in this agent".to_string()),
        request_id: Some(request_id),
    })
}
