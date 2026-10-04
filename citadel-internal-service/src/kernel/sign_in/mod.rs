//! Post-quantum sign-in at the agent (Citadel-Protocol 0.12): the factors a window offers, the
//! security-key relay, management of an account's factors, and the recovery session's limits.
//!
//! The SDK does the cryptography. The agent's part is the boundary: it relays the key touch
//! only a browser can perform, keeps the handle `manage_sign_in` needs, and refuses what a
//! recovery session may not do with an answer instead of the silence the server would give.

pub(crate) mod factors;
pub(crate) mod key_relay;
pub(crate) mod recovery;

use crate::kernel::reconnect::Reauth;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_types::FailureReason;
use citadel_sdk::prelude::{
    CitadelClientServerConnection, NetworkError, PeerChannelRecvHalf, PeerChannelSendHalf, Ratchet,
    SecBuffer, SessionScope,
};
use std::sync::Arc;

/// What a session's sign-in proved, and the SDK handle its management requests go through.
pub(crate) struct SessionSignIn<R: Ratchet> {
    /// The connection the SDK returned, its channel taken by the session's reader and sink.
    /// `manage_sign_in` is defined on it alone.
    pub handle: Arc<CitadelClientServerConnection<R>>,
    pub scope: SessionScope,
}

impl<T, R: Ratchet> CitadelWorkspaceService<T, R> {
    /// Whether `cid` is a session signed in with a recovery code.
    pub(crate) fn is_recovery_session(&self, cid: u64) -> bool {
        self.server_connection_map
            .read()
            .get(&cid)
            .is_some_and(|conn| conn.sign_in.scope == SessionScope::Recovery)
    }
}

/// A new link's send and receive halves, and the connection kept for management.
pub(crate) type Opened<R> = (
    PeerChannelSendHalf<R>,
    PeerChannelRecvHalf<R>,
    Arc<CitadelClientServerConnection<R>>,
);

/// Splits the SDK's connection, keeping it for management. Its UDP receiver is dropped as
/// `split()` dropped it.
pub(crate) fn open<R: Ratchet>(
    mut connected: CitadelClientServerConnection<R>,
) -> Result<Opened<R>, NetworkError> {
    drop(connected.udp_channel_rx.take());
    let channel = connected
        .take_channel()
        .ok_or_else(|| NetworkError::msg("The SDK's connection had no channel"))?;
    let (sink, stream) = channel.split();
    Ok((sink, stream, Arc::new(connected)))
}

/// How the session can be opened again without its user. Only a password-only sign-in can be
/// repeated unprompted: a key needs a touch, and a recovery code signs in once.
pub(crate) fn reauth(scope: SessionScope, key_asked: bool, password: Option<SecBuffer>) -> Reauth {
    match (scope, key_asked, password) {
        (SessionScope::Recovery, _, _) => Reauth::NeedsUser(RECOVERY_REAUTH),
        (SessionScope::Full, true, _) => Reauth::NeedsUser(KEY_REAUTH),
        (SessionScope::Full, false, Some(password)) => Reauth::Password(password),
        (SessionScope::Full, false, None) => Reauth::NeedsUser(KEY_REAUTH),
    }
}

/// The reason a sign-in or registration the server refused carries for the UI, when it has one.
pub(crate) fn failure_reason(code: citadel_io::ErrorCode) -> Option<FailureReason> {
    use citadel_io::ErrorCode;
    match code {
        ErrorCode::PqSignInAdmissionRequired => Some(FailureReason::AdmissionRequired),
        ErrorCode::PqSignInAdmissionFailed => Some(FailureReason::AdmissionFailed),
        _ => None,
    }
}

pub(crate) const KEY_REAUTH: &str =
    "This account signs in with a security key; sign in again to reconnect";
pub(crate) const RECOVERY_REAUTH: &str =
    "A recovery-code session cannot be reconnected; sign in again";

#[cfg(test)]
mod tests {
    use super::*;

    fn is_password(reauth: &Reauth) -> bool {
        matches!(reauth, Reauth::Password(_))
    }

    #[test]
    fn only_a_password_only_sign_in_reconnects_by_itself() {
        let pw = || Some(SecBuffer::from("pw"));
        assert!(is_password(&reauth(SessionScope::Full, false, pw())));
        assert!(
            !is_password(&reauth(SessionScope::Full, true, pw())),
            "a key was touched"
        );
        assert!(
            !is_password(&reauth(SessionScope::Recovery, false, pw())),
            "a recovery code"
        );
        assert!(
            !is_password(&reauth(SessionScope::Full, false, None)),
            "no password at all"
        );
    }
}
