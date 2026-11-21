//! The agent's sign-in wire types: the SDK's JSON exactly, and no credential id under `{:?}`.

use super::*;
use citadel_types::crypto::SecBuffer;

const CRED: &[u8] = b"yubikey-5-credential-id";

fn add() -> SignInManagementOp {
    SignInManagementOp::AddSecurityKey {
        label: "YubiKey".into(),
    }
}

fn credential() -> sdk::SignInCredential {
    sdk::SignInCredential {
        id: 3,
        kind: sdk::FactorKind::SecurityKey,
        label: "YubiKey".into(),
        credential_id: Some(CRED.to_vec()),
        created_ms: 1,
        last_used_ms: Some(2),
        consumed: false,
    }
}

#[test]
fn every_op_crosses_the_wire_as_the_sdk_writes_it() {
    let ops = [
        SignInManagementOp::ListCredentials,
        add(),
        SignInManagementOp::RenameCredential {
            id: 1,
            label: "x".into(),
        },
        SignInManagementOp::RemoveCredential { id: 1 },
        SignInManagementOp::SetSignInPolicy {
            policy: SignInPolicy::KeyOnly,
        },
    ];
    for op in ops {
        let ours = serde_json::to_value(&op).unwrap();
        let sdk = serde_json::to_value(op.to_sdk("acme.example")).unwrap();
        assert_eq!(ours, sdk);
    }
}

/// The two changes whose wire is the agent's own: the new password rides with `SetPassword`
/// (the SDK takes it beside the op), and the phrase's domain is the agent's to derive.
#[test]
fn the_agents_own_ops_name_the_sdks_changes() {
    let set = SignInManagementOp::SetPassword {
        new_password: SecBuffer::from("a new password"),
    };
    assert_eq!(set.to_sdk("x"), sdk::SignInManagementOp::SetPassword);
    assert_eq!(
        SignInManagementOp::RegenerateRecoveryPhrase.to_sdk("acme.example"),
        sdk::SignInManagementOp::RegenerateRecoveryPhrase {
            canonical_domain: "acme.example".into()
        }
    );
}

#[test]
fn a_new_password_never_reaches_a_debug_string() {
    let set = SignInManagementOp::SetPassword {
        new_password: SecBuffer::from("hunter2-new"),
    };
    let printed = format!("{set:?}");
    assert!(!printed.contains("hunter2"), "{printed}");
}

#[test]
fn the_recovery_console_may_do_what_the_sdk_allows_and_nothing_else() {
    let allowed = [
        SignInManagementOp::ListCredentials,
        add(),
        SignInManagementOp::RemoveCredential { id: 1 },
        SignInManagementOp::SetSignInPolicy {
            policy: SignInPolicy::KeyOnly,
        },
        SignInManagementOp::SetPassword {
            new_password: SecBuffer::from("x"),
        },
        SignInManagementOp::RegenerateRecoveryPhrase,
    ];
    for op in allowed {
        assert!(op.allowed_in_recovery(), "{op:?}");
    }
    let rename = SignInManagementOp::RenameCredential {
        id: 1,
        label: "x".into(),
    };
    assert!(!rename.allowed_in_recovery());
}

#[test]
fn every_outcome_crosses_the_wire_as_the_sdk_writes_it() {
    let listed = |policy| sdk::SignInManagementOutcome::Credentials {
        policy,
        credentials: vec![credential()],
    };
    let outcomes = [
        listed(sdk::SignInPolicy::Password),
        listed(sdk::SignInPolicy::PasswordAndKey),
        listed(sdk::SignInPolicy::KeyOnly),
        sdk::SignInManagementOutcome::Added { id: 4 },
        sdk::SignInManagementOutcome::Renamed,
        sdk::SignInManagementOutcome::Removed,
        sdk::SignInManagementOutcome::PolicySet,
        sdk::SignInManagementOutcome::PasswordSet,
        sdk::SignInManagementOutcome::RecoveryPhrase(vec!["abandon".into()]),
    ];
    for outcome in outcomes {
        let sdk = serde_json::to_value(&outcome).unwrap();
        let ours = serde_json::to_value(SignInManagementOutcome::from(outcome)).unwrap();
        assert_eq!(ours, sdk);
    }
}

#[test]
fn no_credential_id_reaches_a_debug_string() {
    let listed = SignInManagementOutcome::from(sdk::SignInManagementOutcome::Credentials {
        policy: sdk::SignInPolicy::KeyOnly,
        credentials: vec![credential()],
    });
    let needle = format!("{:?}", CRED.to_vec());
    let printed = format!("{listed:?}");
    assert!(!printed.contains(&needle), "{printed}");
    assert!(!printed.contains("yubikey-5"), "{printed}");
}

/// Recovery codes are retired: a server's codes outcome, or a listed code, is refused when the
/// SDK decodes it, so neither reaches the agent's conversions.
#[test]
fn recovery_codes_from_a_server_are_refused() {
    let codes = serde_json::json!({ "RecoveryCodes": ["A-B"] });
    assert!(serde_json::from_value::<sdk::SignInManagementOutcome>(codes).is_err());
    let listed_code = serde_json::json!({ "RecoveryCode": null });
    assert!(serde_json::from_value::<sdk::FactorKind>(listed_code).is_err());
    assert!(serde_json::from_value::<sdk::FactorKind>(serde_json::json!("RecoveryCode")).is_err());
}
