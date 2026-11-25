//! A window's app notices (kernel/reminders): one to show now, or the session's reminders for an
//! app, synced whole. The ownership gate (requests/mod.rs) has already refused a connection not
//! attached to the session, so only the session's own windows reach here.

use crate::kernel::reminders::book::acceptable;
use crate::kernel::reminders::runner::ReminderIo;
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    AppNoticeAccepted, AppNoticeFailure, AppNoticeRefusal, InternalServiceRequest,
    InternalServiceResponse,
};
use citadel_sdk::prelude::Ratchet;
use std::sync::Arc;
use uuid::Uuid;

pub(crate) fn failure(
    cid: u64,
    request_id: Uuid,
    reason: AppNoticeRefusal,
) -> InternalServiceResponse {
    InternalServiceResponse::AppNoticeFailure(AppNoticeFailure {
        cid,
        reason,
        request_id: Some(request_id),
    })
}

pub async fn handle<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let (cid, request_id, outcome) = match request {
        InternalServiceRequest::AppNoticePost {
            request_id,
            cid,
            notice,
        } => {
            let outcome = if !acceptable(&notice) || notice.fire_at_ms.is_some() {
                Err(AppNoticeRefusal::BadTarget)
            } else if this.show_app_notice(cid, &notice, false).await {
                Ok(())
            } else {
                Err(AppNoticeRefusal::Unheard)
            };
            (cid, request_id, outcome)
        }
        InternalServiceRequest::AppRemindersSync {
            request_id,
            cid,
            app_id,
            reminders,
        } => {
            let io: Arc<dyn ReminderIo> = Arc::new(this.clone());
            let outcome = this.reminders.sync(io, cid, app_id, reminders).await;
            (cid, request_id, outcome)
        }
        _ => return None,
    };
    let response = match outcome {
        Ok(()) => InternalServiceResponse::AppNoticeAccepted(AppNoticeAccepted {
            cid,
            request_id: Some(request_id),
        }),
        Err(reason) => failure(cid, request_id, reason),
    };
    Some(HandledRequestResult { response, uuid })
}
