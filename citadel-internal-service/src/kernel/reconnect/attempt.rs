//! One reconnect attempt: the SDK connect, with the session's password, bounded by the
//! policy's attempt timeout. Moved out of task.rs unchanged but for the password, which the
//! run takes from the session's `Reauth` before it gets here.

use super::policy::ReconnectPolicy;
use super::Credentials;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_sdk::prelude::{
    AuthenticationRequest, CitadelClientServerConnection, NetworkError, ProtocolRemoteExt, Ratchet,
    SecBuffer,
};

pub(super) async fn once<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    policy: &ReconnectPolicy,
    username: String,
    password: SecBuffer,
    credentials: Credentials,
) -> Result<CitadelClientServerConnection<R>, NetworkError> {
    let connect = this.remote().connect(
        AuthenticationRequest::credentialed(username, password),
        crate::kernel::requests::connect_mode::server_connect_mode(
            credentials.connect_mode,
            crate::kernel::requests::connect_mode::LoginOrigin::AutomaticReconnect,
        ),
        credentials.udp_mode,
        credentials.keep_alive_timeout,
        credentials.session_security_settings,
        credentials.server_password,
    );
    match tokio::time::timeout(policy.attempt_timeout, connect).await {
        Ok(result) => result,
        Err(_) => Err(NetworkError::timeout(policy.attempt_timeout.as_secs())),
    }
}
