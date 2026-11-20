//! `SignInManagement`: list, add, rename or remove an account's sign-in factors, set its policy,
//! set its password, or replace its recovery phrase, through the SDK's `manage_sign_in` and
//! `set_password`.
//!
//! A new phrase is also a new RE-VFS root: its seed becomes the account's root before the window
//! is told, the session stores under its key from then on, and every object stored under the
//! old one is re-keyed (kernel/revfs_seed), now in a full session or at the next full sign-in.
//!
//! The step-up is the window's: its password, and the relay to its windows when it can answer
//! a key challenge (the step-up's own key, or the touch of the key being added). The server
//! judges both; the agent only refuses what a recovery session may not ask (requests/mod.rs).

use crate::kernel::requests::HandledRequestResult;
use crate::kernel::revfs_seed;
use crate::kernel::sign_in::key_relay::Asker;
use crate::kernel::sign_in::{factors, management_failure_reason};
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    FailureReason, InternalServiceRequest, InternalServiceResponse, RecoveryCodes,
    SignInManagementFailure, SignInManagementOp, SignInManagementOutcome, SignInManagementSuccess,
};
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::{Ratchet, SignInManagementExt, SignInManagementOutcome as SdkOutcome};
use uuid::Uuid;

pub async fn handle<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::SignInManagement {
        request_id,
        cid,
        op,
        step_up,
    } = request
    else {
        unreachable!("Should never happen if programmed properly")
    };
    let failure = |message: String, reason: Option<FailureReason>| {
        let response = InternalServiceResponse::SignInManagementFailure(SignInManagementFailure {
            cid,
            request_id: Some(request_id),
            message,
            reason,
        });
        Some(HandledRequestResult { response, uuid })
    };

    let session = {
        let map = this.server_connection_map.read();
        map.get(&cid)
            .map(|conn| (conn.sign_in.handle.clone(), conn.subscribers.members()))
    };
    let Some((handle, windows)) = session else {
        return failure(format!("Session {cid} not found"), None);
    };

    let asker = Asker {
        audience: windows,
        cid,
        request_id,
    };
    let (challenges, clients) = (&this.key_challenges, &this.tx_to_localhost_clients);
    let password = step_up.password;
    let (factors, underway) = factors::begin(
        challenges,
        clients,
        asker,
        factors::Offer {
            password,
            security_key: step_up.security_key,
            recovery_phrase: None,
            admission: None,
        },
    );

    let kind = op_name(&op);
    let adds_a_key = matches!(op, SignInManagementOp::AddSecurityKey { .. });
    // The phrase is bound to the account's server, derived here as at registration.
    let domain = match op {
        SignInManagementOp::RegenerateRecoveryPhrase => {
            match crate::kernel::recovery_domain::of_account(this.remote(), cid).await {
                Ok(domain) => domain.to_string(),
                Err(message) => return failure(message, None),
            }
        }
        _ => String::new(),
    };
    let outcome = match op {
        SignInManagementOp::SetPassword { new_password } => handle
            .set_password(new_password, factors)
            .await
            .map(|()| SdkOutcome::PasswordSet),
        op => handle.manage_sign_in(op.to_sdk(&domain), factors).await,
    };
    drop(underway);
    match outcome {
        Ok(outcome) => {
            info!(target: "citadel", "[SignIn] {kind} for session {cid} succeeded");
            let outcome = match SignInManagementOutcome::try_from(outcome) {
                Ok(outcome) => outcome,
                Err(unsupported) => return failure(unsupported.to_string(), None),
            };
            if let SignInManagementOutcome::RecoveryPhrase(words) = &outcome {
                new_root(this, cid, words).await;
            }
            let response =
                InternalServiceResponse::SignInManagementSuccess(SignInManagementSuccess {
                    cid,
                    request_id: Some(request_id),
                    outcome,
                });
            Some(HandledRequestResult { response, uuid })
        }
        Err(err) => {
            info!(target: "citadel", "[SignIn] {kind} for session {cid} was refused");
            let reason = management_failure_reason(adds_a_key, err.code);
            failure(err.into_string(), reason)
        }
    }
}

/// The regenerated phrase's seed becomes the account's root, the session's key follows it, and
/// the objects sealed under the old root are re-keyed. The phrase already replaced the old one
/// at the server, so a failure here cannot refuse it: the window is still given the words, and
/// the account's files stay under the old root, which this device keeps.
async fn new_root<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    words: &RecoveryCodes,
) {
    let rotated = match revfs_seed::seed_of_words(&words.0) {
        Ok(seed) => revfs_seed::rotate(this.revfs_vault.get(), this.remote(), cid, &seed).await,
        Err(err) => Err(err),
    };
    match rotated {
        Ok(key) => {
            if let Some(conn) = this.server_connection_map.write().get_mut(&cid) {
                conn.revfs_key = key;
            }
            revfs_seed::resume(this, cid);
        }
        Err(err) => {
            warn!(target: "citadel", "[SignIn] Account {cid}'s new phrase is not its RE-VFS root here: {err}");
        }
    }
}

/// The change's name, for the log: never its labels or credential ids.
fn op_name(op: &SignInManagementOp) -> &'static str {
    use SignInManagementOp as Op;
    match op {
        Op::ListCredentials => "ListCredentials",
        Op::AddSecurityKey { .. } => "AddSecurityKey",
        Op::RenameCredential { .. } => "RenameCredential",
        Op::RemoveCredential { .. } => "RemoveCredential",
        Op::SetSignInPolicy { .. } => "SetSignInPolicy",
        Op::SetPassword { .. } => "SetPassword",
        Op::RegenerateRecoveryPhrase => "RegenerateRecoveryPhrase",
    }
}
