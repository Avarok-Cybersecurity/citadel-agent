//! `RecoverAccount`: recover an account with its recovery phrase, then sign in with the phrase to
//! the recovery console.
//!
//! On a device that holds no bundle for the account, the SDK's `recover` installs this device's
//! in place of every other device's (they are signed out), and the agent records the server host
//! as a registration does. On a device that already holds it, nothing is revoked: the phrase only
//! signs in. Either way the phrase sign-in is the ordinary `Connect`, whose answer comes last.

use crate::kernel::account_server::Refusal;
use crate::kernel::requests::{handle_request, HandledRequestResult};
use crate::kernel::server_address::ServerAddress;
use crate::kernel::session_route::deliver;
use crate::kernel::sign_in::{admission_challenge, failure_reason, phrase};
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, RecoverAccountFailure, RecoverAccountSuccess,
};
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::{AccountRecovery, AccountRecoveryExt, ConnectMode, Ratchet};
use uuid::Uuid;

/// The SDK's bound on a device label (`AccountRecovery::device_label`).
const MAX_LABEL_CHARS: usize = 64;

pub async fn handle<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::RecoverAccount {
        request_id,
        server_addr,
        username,
        phrase: typed,
        device_label,
        admission_token,
        session_security_settings,
        server_password,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };
    let refuse = |message: String| {
        let response = InternalServiceResponse::RecoverAccountFailure(RecoverAccountFailure {
            cid: 0,
            request_id: Some(request_id),
            message,
            reason_code: None,
            admission_challenge: None,
        });
        Some(HandledRequestResult { response, uuid })
    };
    let remote = this.remote();

    let address = match crate::kernel::server_address::classify(&server_addr) {
        Err(err) => return refuse(format!("invalid server address {server_addr}: {err}")),
        Ok(address) => address,
    };
    let server_host = match crate::kernel::server_host::from_typed(&address) {
        Err(err) => return refuse(format!("invalid server address {server_addr}: {err}")),
        Ok(server_host) => server_host,
    };
    let domain = match crate::kernel::recovery_domain::of_server(&server_host) {
        Err(err) => return refuse(format!("Cannot recover at {server_addr}: {err}")),
        Ok(domain) => domain,
    };
    let phrase = match phrase::read(&typed, &domain) {
        Err(message) => return refuse(message),
        Ok(phrase) => phrase,
    };
    let device_label = match label(&device_label) {
        Err(message) => return refuse(message),
        Ok(label) => label,
    };

    let held = match crate::kernel::account_server::check(remote, &username, &server_host).await {
        Ok(()) => true,
        Err(Refusal::NoSuchAccount) => false,
        Err(Refusal::OnAnotherServer(held)) => {
            return refuse(format!(
                "This device already has an account named {username} on {held}; one device holds one account per username"
            ))
        }
        Err(Refusal::InvalidServer(err)) => {
            return refuse(format!(
                "could not check this device's accounts for {username}: {err}"
            ))
        }
    };

    if !held {
        let recovery = AccountRecovery {
            username: username.clone(),
            phrase,
            device_label,
            admission: admission_token,
        };
        let certificate = crate::kernel::server_certificate::SERVER_CERTIFICATE;
        info!(target: "citadel", "[Recover] Recovering {username} at {server_host}");
        let recovered = match address {
            ServerAddress::WebSocket(endpoint) => {
                remote
                    .recover_at_endpoint(
                        endpoint,
                        recovery,
                        session_security_settings,
                        server_password.clone(),
                        certificate,
                    )
                    .await
            }
            ServerAddress::HostPort(host_port) => {
                let addr = match tokio::net::lookup_host(&host_port).await {
                    Ok(mut addrs) => match addrs.next() {
                        Some(addr) => addr,
                        None => return refuse(format!("{host_port} resolved to no addresses")),
                    },
                    Err(err) => return refuse(format!("could not resolve {host_port}: {err}")),
                };
                remote
                    .recover(
                        addr,
                        recovery,
                        session_security_settings,
                        server_password.clone(),
                        certificate,
                    )
                    .await
            }
        };
        let cid = match recovered {
            Ok(recovered) => recovered.cid,
            Err(err) => {
                info!(target: "citadel", "[Recover] Recovery of {username} was refused");
                let response =
                    InternalServiceResponse::RecoverAccountFailure(RecoverAccountFailure {
                        cid: 0,
                        request_id: Some(request_id),
                        reason_code: failure_reason(err.code),
                        admission_challenge: admission_challenge(&err),
                        message: err.into_string(),
                    });
                return Some(HandledRequestResult { response, uuid });
            }
        };
        // As a registration does, so the phrase sign-in below names the account's own server.
        if let Err(err) = crate::kernel::server_host::store(remote, cid, &server_host).await {
            warn!(target: "citadel", "[Recover] Recovered {cid} but could not record its server host {server_host}: {err}");
        }
        let response = InternalServiceResponse::RecoverAccountSuccess(RecoverAccountSuccess {
            cid,
            request_id: Some(request_id),
        });
        if deliver(&this.tx_to_localhost_clients, &[uuid], response).is_empty() {
            warn!(target: "citadel", "[Recover] {cid} recovered, but its window left before it was told");
        }
    }

    let connect = InternalServiceRequest::Connect {
        request_id,
        username,
        server_address: server_host,
        password: None,
        security_key: false,
        recovery_code: None,
        recovery_phrase: Some(typed),
        admission_token: None,
        connect_mode: ConnectMode::Standard { force_login: false },
        udp_mode: Default::default(),
        keep_alive_timeout: None,
        session_security_settings,
        server_password,
    };
    handle_request(this, uuid, connect).await
}

/// The device's label, trimmed: 1 to 64 characters.
fn label(typed: &str) -> Result<String, String> {
    let trimmed = typed.trim();
    match trimmed.chars().count() {
        0 => Err("Name this device so its sign-ins can be told apart".to_string()),
        n if n > MAX_LABEL_CHARS => Err(format!(
            "A device name is at most {MAX_LABEL_CHARS} characters"
        )),
        _ => Ok(trimmed.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::label;

    #[test]
    fn a_device_label_is_one_to_sixty_four_characters_trimmed() {
        assert_eq!(label("  Laptop  ").unwrap(), "Laptop");
        assert!(label("   ").is_err());
        assert!(label(&"é".repeat(64)).is_ok());
        assert!(label(&"é".repeat(65)).is_err());
    }
}
