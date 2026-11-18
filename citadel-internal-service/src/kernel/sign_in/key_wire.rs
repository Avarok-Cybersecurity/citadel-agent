//! The SDK's security-key request in the agent's wire: what a `SecurityKeyChallengeNotification`
//! carries. Every variant is matched with no catch-all, so one the SDK adds is a compile error.

use citadel_internal_service_types::{SecurityKeyCeremony, SecurityKeyPurpose};
use citadel_proto::auth::{SecurityKeyCeremony as SdkCeremony, SecurityKeyPurpose as SdkPurpose};

pub(super) fn wire_ceremony(ceremony: &SdkCeremony) -> SecurityKeyCeremony {
    match ceremony {
        SdkCeremony::Authenticate { credential_ids } => SecurityKeyCeremony::Authenticate {
            credential_ids: credential_ids.clone(),
        },
        SdkCeremony::Register {
            user_id,
            user_name,
            exclude_credential_ids,
        } => SecurityKeyCeremony::Register {
            user_id: user_id.clone(),
            user_name: user_name.clone(),
            exclude_credential_ids: exclude_credential_ids.clone(),
        },
    }
}

pub(super) fn purpose(purpose: SdkPurpose) -> SecurityKeyPurpose {
    match purpose {
        SdkPurpose::SignIn => SecurityKeyPurpose::SignIn,
        SdkPurpose::StepUp => SecurityKeyPurpose::StepUp,
        SdkPurpose::Enrol => SecurityKeyPurpose::Enrol,
    }
}
