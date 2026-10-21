//! A window's console lines for its session's opt-in diagnostic logs (telemetry/browser.rs).
//!
//! The ownership gate has already confirmed the caller is one of the session's windows. The
//! collector decides the rest: nothing is kept for a session that has not opted in. Nothing is
//! answered and nothing is logged here: a line logged per request would itself be captured.

use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_types::InternalServiceRequest;
use citadel_sdk::prelude::Ratchet;

pub(crate) fn handle<T, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    command: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    if let (InternalServiceRequest::BrowserConsole { cid, entries, .. }, Some(uplink)) =
        (command, this.telemetry.as_ref())
    {
        uplink.browser_console(cid, &entries);
    }
    None
}
