//! `SignInManagement`: list, add, rename or remove an account's sign-in factors, set its policy,
//! set its password, or replace its recovery phrase, through the SDK's `change_sign_in`.
//!
//! A new phrase is also a new RE-VFS root: its seed becomes the account's root before the window
//! is told, the session stores under its key from then on, and every object stored under the
//! old one is re-keyed (kernel/revfs_seed), now in a full session or at the next full sign-in.
//!
//! The step-up is the window's: its password, and the relay to its windows when it can answer
//! a key challenge (the step-up's own key, or the touch of the key being added). The server
//! judges both; the agent only refuses what a recovery session may not ask (requests/mod.rs).

use crate::kernel::reconnect::Reauth;
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::revfs_seed;
use crate::kernel::sign_in::key_relay::Asker;
use crate::kernel::sign_in::reauth_change::{self, Change, Fingerprint};
use crate::kernel::sign_in::{factors, management_failure_reason};
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    FailureReason, InternalServiceRequest, InternalServiceResponse, RecoveryCodes,
    SignInManagementFailure, SignInManagementOp, SignInManagementOutcome, SignInManagementSuccess,
};
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::{
    Ratchet, ResumeCredential, SignInManagementExt, SignInManagementOp as SdkOp,
};
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
            resume_lifetime_s: None,
        },
    );
    // The resume credential this session holds goes with the change, so the server hands it
    // back bound to the account as the change leaves it (kernel/resume).
    let mut factors = factors;
    factors.resume = resume_held(this, cid);

    let kind = op_name(&op);
    let adds_a_key = matches!(op, SignInManagementOp::AddSecurityKey { .. });
    let change = Change::of(&op);
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
    let (sdk_op, new_password) = match op {
        SignInManagementOp::SetPassword { new_password } => {
            (SdkOp::SetPassword, Some(new_password))
        }
        op => (op.to_sdk(&domain), None),
    };
    let changed = handle.change_sign_in(sdk_op, new_password, factors).await;
    drop(underway);
    match changed {
        Ok(changed) => {
            info!(target: "citadel", "[SignIn] {kind} for session {cid} succeeded");
            let outcome = SignInManagementOutcome::from(changed.outcome);
            keep_what_now_signs_in(this, cid, change, changed.resume).await;
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

/// The resume credential `cid`'s session signs in again with, if it holds one.
fn resume_held<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
) -> Option<ResumeCredential> {
    let map = this.server_connection_map.read();
    match &map.get(&cid)?.reconnect.reauth {
        Reauth::Resume(credential) => Some(credential.clone()),
        Reauth::Password(_) | Reauth::NeedsUser(_) => None,
    }
}

/// The server accepted a change: the session keeps what opens it from now on
/// (sign_in/reauth_change.rs), and its fingerprint follows, so a window can reach it on the
/// password only while the password alone still signs the account in. `reissued` is the
/// session's resume credential as the server re-bound it to the changed account.
async fn keep_what_now_signs_in<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    change: Option<Change>,
    reissued: Option<ResumeCredential>,
) {
    let (username, fingerprint) = {
        let mut map = this.server_connection_map.write();
        let Some(conn) = map.get_mut(&cid) else {
            return;
        };
        let fingerprint = reauth_change::fingerprint_after(&conn.reconnect.reauth, change.as_ref());
        let current = conn.reconnect.reauth.clone();
        conn.reconnect.reauth = reauth_change::kept_after(current, change, reissued);
        (conn.username.clone(), fingerprint)
    };
    let fingerprint = match fingerprint {
        Fingerprint::Unchanged => return,
        Fingerprint::None => None,
        Fingerprint::Of(password) => {
            crate::kernel::credential_fingerprint::derive(this.remote(), &username, password).await
        }
    };
    if let Some(conn) = this.server_connection_map.write().get_mut(&cid) {
        conn.credential_fingerprint = fingerprint;
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
