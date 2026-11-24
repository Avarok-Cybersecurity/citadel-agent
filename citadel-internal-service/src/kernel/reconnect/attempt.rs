//! One reconnect attempt: the SDK connect, with what the session kept to sign in again,
//! bounded by the policy's attempt timeout.

use super::policy::ReconnectPolicy;
use super::{Credentials, Reauth};
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_sdk::prelude::{
    AuthenticationRequest, CitadelClientServerConnection, NetworkError, ProtocolRemoteExt, Ratchet,
    SignInFactors,
};

/// The sign-in a reconnect makes for `username` with what its session kept, or the reason it
/// cannot make one without the user. A resume credential signs in by itself; the SDK refuses
/// an expired one by name before it dials.
pub(super) fn sign_in_again(
    username: String,
    reauth: &Reauth,
) -> Result<AuthenticationRequest, &'static str> {
    match reauth {
        Reauth::Password(password) => Ok(AuthenticationRequest::credentialed(
            username,
            password.clone(),
        )),
        Reauth::Resume(credential) => Ok(AuthenticationRequest::sign_in(
            username,
            SignInFactors::resume(credential.clone()),
        )),
        Reauth::NeedsUser(reason) => Err(reason),
    }
}

pub(super) async fn once<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    policy: &ReconnectPolicy,
    auth: AuthenticationRequest,
    credentials: Credentials,
) -> Result<CitadelClientServerConnection<R>, NetworkError> {
    let connect = this.remote().connect(
        auth,
        crate::kernel::requests::connect_mode::server_connect_mode(
            credentials.connect_mode,
            crate::kernel::requests::connect_mode::LoginOrigin::AutomaticReconnect,
        ),
        credentials.udp_mode,
        credentials.keep_alive_timeout,
        credentials.session_security_settings,
        credentials.server_password,
        this.server_certificate,
    );
    match tokio::time::timeout(policy.attempt_timeout, connect).await {
        Ok(result) => result,
        Err(_) => Err(NetworkError::timeout(policy.attempt_timeout.as_secs())),
    }
}
