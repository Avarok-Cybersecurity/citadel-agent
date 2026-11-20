//! What a recovery console session (signed in with the recovery phrase or a code) may ask the
//! agent for.
//!
//! The server already confines such a session: it drops every packet but connect, keep-alive,
//! disconnect and peer commands, and refuses every signal but the two management changes a
//! recovery allows. Dropped is silent, though, so a window asking it for peers or messages
//! would wait out its own timeout. The agent refuses those at once, saying why.

use citadel_internal_service_types::InternalServiceRequest;

pub(crate) const RECOVERY_ONLY: &str = "This session signed in to recover the account: it can \
     only manage how the account signs in, or sign out";

/// Whether a recovery session may make this request.
pub(crate) fn allows(command: &InternalServiceRequest) -> bool {
    match command {
        InternalServiceRequest::Disconnect { .. } => true,
        // The SDK's rule, not a copy of it.
        InternalServiceRequest::SignInManagement { op, .. } => op.allowed_in_recovery(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use citadel_internal_service_types::StepUp;
    use citadel_internal_service_types::{SignInManagementOp, SignInPolicy};
    use uuid::Uuid;

    fn manage(op: SignInManagementOp) -> InternalServiceRequest {
        InternalServiceRequest::SignInManagement {
            request_id: Uuid::nil(),
            cid: 1,
            op,
            step_up: StepUp {
                password: None,
                security_key: true,
            },
        }
    }

    #[test]
    fn a_recovery_session_may_manage_its_sign_in_methods_and_sign_out() {
        let add = SignInManagementOp::AddSecurityKey {
            label: "key".into(),
        };
        let policy = SignInManagementOp::SetSignInPolicy {
            policy: SignInPolicy::KeyOnly,
        };
        assert!(allows(&manage(add)));
        assert!(allows(&manage(policy)));
        assert!(allows(&manage(SignInManagementOp::ListCredentials)));
        assert!(allows(&manage(
            SignInManagementOp::RegenerateRecoveryPhrase
        )));
        assert!(allows(&manage(SignInManagementOp::SetPassword {
            new_password: "a new password".into(),
        })));
        assert!(allows(&manage(SignInManagementOp::RemoveCredential {
            id: 1
        })));
        let disconnect = InternalServiceRequest::Disconnect {
            request_id: Uuid::nil(),
            cid: 1,
        };
        assert!(allows(&disconnect));
    }

    #[test]
    fn and_nothing_else() {
        for op in [
            SignInManagementOp::RenameCredential {
                id: 1,
                label: "x".into(),
            },
            SignInManagementOp::RegenerateRecoveryCodes,
        ] {
            assert!(!allows(&manage(op.clone())), "{op:?}");
        }
        let peers = InternalServiceRequest::ListRegisteredPeers {
            request_id: Uuid::nil(),
            cid: 1,
        };
        assert!(!allows(&peers));
    }
}
