//! The agent's sign-in wire types: the SDK's JSON exactly, and no credential id under `{:?}`.

use super::*;

const CRED: &[u8] = b"yubikey-5-credential-id";

fn add() -> SignInManagementOp {
    SignInManagementOp::AddSecurityKey {
        credential_id: CRED.to_vec(),
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
        SignInManagementOp::RegenerateRecoveryCodes,
    ];
    for op in ops {
        let ours = serde_json::to_value(&op).unwrap();
        let sdk = serde_json::to_value(sdk::SignInManagementOp::from(op)).unwrap();
        assert_eq!(ours, sdk);
    }
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
        sdk::SignInManagementOutcome::RecoveryCodes(vec!["A-B".into()]),
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
    for printed in [format!("{listed:?}"), format!("{:?}", add())] {
        assert!(!printed.contains(&needle), "{printed}");
        assert!(!printed.contains("yubikey-5"), "{printed}");
    }
}
