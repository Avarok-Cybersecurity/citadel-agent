//! What a session signed in with a recovery code may ask the agent for.
//!
//! The server already confines such a session: it drops every packet but connect, keep-alive,
//! disconnect and peer commands, and refuses every signal but the two management changes a
//! recovery allows. Dropped is silent, though, so a window asking it for peers or messages
//! would wait out its own timeout. The agent refuses those at once, saying why.

use citadel_internal_service_types::InternalServiceRequest;

pub(crate) const RECOVERY_ONLY: &str = "This session signed in with a recovery code: it can \
     only add a security key, set the sign-in policy, or sign out";

/// Whether a recovery session may make this request.
pub(crate) fn allows(command: &InternalServiceRequest) -> bool {
    match command {
        InternalServiceRequest::Disconnect { .. } => true,
        InternalServiceRequest::SignInManagement { op, .. } => op.allowed_in_recovery(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use citadel_internal_service_types::StepUp;
    use citadel_sdk::prelude::{SignInManagementOp, SignInPolicy};
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
    fn a_recovery_session_may_add_a_key_set_the_policy_and_sign_out() {
        let add = SignInManagementOp::AddSecurityKey {
            credential_id: vec![1],
            label: "key".into(),
        };
        let policy = SignInManagementOp::SetSignInPolicy {
            policy: SignInPolicy::KeyOnly,
        };
        assert!(allows(&manage(add)));
        assert!(allows(&manage(policy)));
        let disconnect = InternalServiceRequest::Disconnect {
            request_id: Uuid::nil(),
            cid: 1,
        };
        assert!(allows(&disconnect));
    }

    #[test]
    fn and_nothing_else() {
        for op in [
            SignInManagementOp::ListCredentials,
            SignInManagementOp::RemoveCredential { id: 1 },
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
