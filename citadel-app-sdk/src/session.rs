//! `session` (`citadel.session`): a C2S session to an external Citadel server the manifest names
//! in `servers`, held by the member's agent. [`open`] answers the session's number and state;
//! what happens on it arrives as `AppEvent::Session { session, event }` in the app's `event`
//! (with the `events` subscription, as topics). Check `AppContext::is_granted` for
//! `Capability::CitadelSession` before opening, and fall back to `net.host` without it.

pub use citadel_app_sdk_types::calls::session::{
    LIMIT_SESSION_BYTES, LIMIT_SESSION_RATE, MAX_SESSION_SERVERS,
};
pub use citadel_app_sdk_types::calls::{SessionEvent, SessionState, SessionStatus};

use crate::abi::RawAbi;
use crate::pending::{start, Pending};
use citadel_app_sdk_types::calls::{SessionClose, SessionOpen, SessionSend};
use citadel_app_sdk_types::{Bytes, Refused};

pub(crate) fn open_with(raw: &impl RawAbi, server: &str) -> Pending<SessionState> {
    start(
        raw,
        &SessionOpen {
            server: server.into(),
        },
    )
}

pub(crate) fn send_with(raw: &impl RawAbi, session: u32, body: &[u8]) -> Result<(), Refused> {
    let send = SessionSend {
        session,
        body: Bytes(body.to_vec()),
    };
    let mut sent = start(raw, &send);
    match sent.take_ready() {
        Some(result) => result,
        None => {
            sent.watch("session send");
            Ok(())
        }
    }
}

pub(crate) fn close_with(raw: &impl RawAbi, session: u32) {
    start(raw, &SessionClose { session }).watch("session close");
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::*;
    use crate::ffi::WASM;

    /// Open the session to `server` (one of the manifest's `servers`), or get the one already
    /// open to it. A server outside the manifest is `ungranted`, answered at once.
    pub fn open(server: &str) -> Pending<SessionState> {
        open_with(&WASM, server)
    }

    /// One body to the server. A size (`session_bytes`) or rate (`session_rate`) refusal, or an
    /// unknown or closed session, is answered at once; a later failure is logged.
    pub fn send(session: u32, body: &[u8]) -> Result<(), Refused> {
        send_with(&WASM, session, body)
    }

    pub fn close(session: u32) {
        close_with(&WASM, session)
    }
}

#[cfg(target_arch = "wasm32")]
pub use wasm::*;

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
