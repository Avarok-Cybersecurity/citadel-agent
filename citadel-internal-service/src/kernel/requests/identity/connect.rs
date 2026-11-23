//! `ConnectIdentity` (identity doc §4.3, `server-identity-challenge.md` §3): sign in by the
//! account's pinned username and domain, never recomputed, with the derived password; then
//! challenge the server. Any failure of the challenge disconnects the session, tells the window
//! with both fingerprints, and leaves the account and its pin as they were.

use super::host;
use crate::kernel::identity::pin::{self, Pin};
use crate::kernel::identity::{IdentityHost, Refused};
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_identity::{parse_username, Identity, Username};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    ConnectFailure, FailureReason, IdentityOutcome, IdentityRefusal, InternalServiceRequest,
    InternalServiceResponse, SessionSecuritySettings,
};
use citadel_sdk::logging::warn;
use citadel_sdk::prelude::{ConnectMode, PreSharedKey, Ratchet};
use std::sync::Arc;
use uuid::Uuid;

pub(crate) async fn handle<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    command: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::ConnectIdentity {
        request_id,
        cid,
        session_security_settings,
        server_password,
        resume_lifetime_s,
    } = command
    else {
        unreachable!("dispatched by requests/mod.rs")
    };
    connect_identity(
        this,
        uuid,
        request_id,
        cid,
        session_security_settings,
        server_password,
        resume_lifetime_s,
    )
    .await
}

/// The unlocked identity, account `cid`'s pin, and its username, which must be the pin's
/// handle at the pin's domain and must have been registered by this identity.
async fn account<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
) -> Result<(Arc<IdentityHost>, Arc<Identity>, Pin, String), (Option<FailureReason>, String)> {
    let host = host(this).map_err(|(_, message)| (None, message))?;
    let identity = host.unlocked.current().ok_or((
        Some(FailureReason::IdentityLocked),
        "Unlock your identity to sign in".to_string(),
    ))?;
    let not_identity = |message: String| (Some(FailureReason::NotAnIdentityAccount), message);
    let remote = this.remote();
    let pin = pin::load(remote, cid)
        .await
        .map_err(|err| (None, err))?
        .ok_or_else(|| {
            not_identity(format!("account {cid} was not registered with an identity"))
        })?;
    if pin.id_fp != identity.id_fp() {
        return Err(not_identity(format!(
            "account {cid} belongs to identity {}, not the unlocked {}",
            pin.id_fp,
            identity.id_fp()
        )));
    }
    let queried = remote
        .account_manager()
        .get_username_by_cid(cid)
        .await
        .map_err(|err| err.into_string());
    let username = account_username(cid, queried)?;
    let pinned = Username::V2 {
        handle: &pin.handle,
        domain: pin.domain.as_str(),
    };
    if parse_username(&username) != Ok(pinned) {
        return Err(not_identity(format!(
            "{username} is not the pinned {}@{}",
            pin.handle, pin.domain
        )));
    }
    Ok((host, identity, pin, username))
}

/// Account `cid`'s username, from the store's answer. A failed read is reported as one, never
/// as "no such account": the account may well be there.
fn account_username(
    cid: u64,
    queried: Result<Option<String>, String>,
) -> Result<String, (Option<FailureReason>, String)> {
    match queried {
        Ok(Some(username)) => Ok(username),
        Ok(None) => Err((
            Some(FailureReason::NoSuchAccount),
            format!("this computer has no account {cid}"),
        )),
        Err(err) => Err((
            None,
            format!("could not read account {cid}'s username: {err}"),
        )),
    }
}

pub(crate) async fn connect_identity<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request_id: Uuid,
    cid: u64,
    session_security_settings: SessionSecuritySettings,
    server_password: Option<PreSharedKey>,
    resume_lifetime_s: Option<u32>,
) -> Option<HandledRequestResult> {
    let fail = |reason: Option<FailureReason>, message: String| {
        let response = InternalServiceResponse::ConnectFailure(ConnectFailure {
            cid,
            message,
            request_id: Some(request_id),
            reason_code: reason,
            admission_challenge: None,
        });
        Some(HandledRequestResult { response, uuid })
    };
    let (host, identity, pin, username) = match account(this, cid).await {
        Ok(found) => found,
        Err((reason, message)) => return fail(reason, message),
    };
    let remote = this.remote();
    // The server the account was registered to, as recorded then: never recomputed.
    let server_address = match crate::kernel::server_host::load(remote, cid).await {
        Ok(Some(host)) => host,
        Ok(None) => return fail(None, format!("account {cid} recorded no server")),
        Err(err) => return fail(None, err),
    };
    let password = identity.derive_password(&pin.domain, &username);
    // As a Register's own connect-after-register (requests/register.rs).
    let connect = InternalServiceRequest::Connect {
        resume_lifetime_s,
        request_id,
        username,
        server_address,
        password: Some(password.as_str().as_bytes().to_vec().into()),
        security_key: false,
        recovery_phrase: None,
        admission_token: None,
        connect_mode: ConnectMode::Standard { force_login: false },
        udp_mode: Default::default(),
        keep_alive_timeout: None,
        session_security_settings,
        server_password,
    };
    let connected = crate::kernel::requests::connect::handle(this, uuid, connect).await?;
    let InternalServiceResponse::ConnectSuccess(mut success) = connected.response else {
        return Some(connected);
    };
    // The identity signs the session's P2P key exchanges for as long as the session lasts,
    // whether or not the identity stays unlocked (kernel/p2p_kex).
    if let Some(session) = this.server_connection_map.write().get_mut(&cid) {
        session.p2p_kex_signer = Some(identity.p2p_kex_signer());
    }
    match super::post_connect::challenge(this, &host, &pin, cid).await {
        Ok(key) => {
            let proved = Pin {
                server_pk: Some(key),
                ..pin.clone()
            };
            // A `Pinned` registration's fingerprint is now proved by its key: recorded once.
            if pin.server_pk.is_none() {
                if let Err(err) = pin::store(remote, cid, &proved).await {
                    warn!(target: "citadel", "[ConnectIdentity] {cid}: the challenge passed but the proved key was not recorded: {err}");
                }
            }
            let wire = proved.wire();
            host.record_session(cid, wire.clone());
            success.identity = Some(wire);
            Some(HandledRequestResult {
                response: InternalServiceResponse::ConnectSuccess(success),
                uuid,
            })
        }
        Err(mismatch) => {
            super::post_connect::refuse_server(this, uuid, cid, &pin, &mismatch).await;
            fail(
                Some(FailureReason::ServerIdentityMismatch),
                format!(
                    "{} did not prove its pinned key: {}",
                    pin.domain, mismatch.why
                ),
            )
        }
    }
}

/// `IdentityBinding`: the identity's signature that it holds account `cid`'s username on the
/// pinned domain, issued now.
pub(crate) async fn binding<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
) -> Result<IdentityOutcome, Refused> {
    let (_, identity, pin, username) = account(this, cid).await.map_err(|(reason, message)| {
        let refusal = match reason {
            Some(FailureReason::IdentityLocked) => IdentityRefusal::Locked,
            _ => IdentityRefusal::NotAnIdentityAccount,
        };
        (refusal, message)
    })?;
    let now_s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|err| (IdentityRefusal::Storage, err.to_string()))?
        .as_secs();
    let blob = identity
        .sign_binding(&username, &pin.domain, now_s)
        .map_err(|err| (IdentityRefusal::NotAnIdentityAccount, err.to_string()))?;
    Ok(IdentityOutcome::Binding {
        cid,
        binding: blob.to_base64url(),
    })
}

#[cfg(test)]
mod tests {
    //! Control: map `Err` to the `Ok(None)` arm in `account_username` → a failed read is
    //! reported as a missing account and `a_failed_username_read_is_not_a_missing_account`
    //! goes red.
    use super::*;

    #[test]
    fn a_failed_username_read_is_not_a_missing_account() {
        assert_eq!(
            account_username(7, Ok(Some("alice@a.example".into()))).unwrap(),
            "alice@a.example"
        );
        let missing = account_username(7, Ok(None)).unwrap_err();
        assert_eq!(missing.0, Some(FailureReason::NoSuchAccount));
        let failed = account_username(7, Err("backend down".into())).unwrap_err();
        assert_eq!(
            failed.0, None,
            "a failed read names no reason the UI would act on"
        );
        assert!(failed.1.contains("backend down"), "{}", failed.1);
    }
}
