//! The WebAuthn ceremony a `SecurityKeyChallengeNotification` asks a window to run: the agent's
//! wire form of the SDK's `SecurityKeyCeremony`, field for field.
//!
//! A sign-in or a step-up authenticates with a key the account already has. Adding a key
//! registers a new credential, which the SDK asks for once the server has named the relying
//! party and the keys the account already holds; the window creates it and answers with its id.

use crate::bytes_debug_fmt;
use custom_debug::Debug;
use serde::{Deserialize, Serialize};

#[cfg(feature = "typescript")]
use ts_rs::TS;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum SecurityKeyCeremony {
    /// WebAuthn `get` with `allowCredentials = credential_ids`.
    Authenticate {
        #[debug(with = credential_ids_fmt)]
        #[cfg_attr(feature = "typescript", ts(type = "Array<Uint8Array>"))]
        #[serde(with = "crate::wire_bytes::seq")]
        credential_ids: Vec<Vec<u8>>,
    },
    /// WebAuthn `create` of a non-discoverable credential with the PRF extension enabled, for
    /// `user_name` with user handle `user_id`, excluding `exclude_credential_ids`; then a `get`
    /// of the new credential for its PRF output.
    Register {
        #[debug(with = bytes_debug_fmt)]
        #[cfg_attr(feature = "typescript", ts(type = "Uint8Array"))]
        #[serde(with = "crate::wire_bytes")]
        user_id: Vec<u8>,
        user_name: String,
        #[debug(with = credential_ids_fmt)]
        #[cfg_attr(feature = "typescript", ts(type = "Array<Uint8Array>"))]
        #[serde(with = "crate::wire_bytes::seq")]
        exclude_credential_ids: Vec<Vec<u8>>,
    },
}

impl SecurityKeyCeremony {
    /// Whether an answer naming `credential_id` is one this ceremony asked for: one of the
    /// allowed keys when authenticating; a new, non-empty id that is none of the account's keys
    /// when registering. The SDK applies the same rule once the answer reaches it.
    pub fn accepts(&self, credential_id: &[u8]) -> bool {
        match self {
            Self::Authenticate { credential_ids } => {
                credential_ids.iter().any(|id| id == credential_id)
            }
            Self::Register {
                exclude_credential_ids,
                ..
            } => {
                !credential_id.is_empty()
                    && !exclude_credential_ids.iter().any(|id| id == credential_id)
            }
        }
    }
}

/// How many credentials a ceremony names, never which.
#[allow(clippy::ptr_arg)] // custom_debug hands the field over as `&Vec`.
fn credential_ids_fmt(ids: &Vec<Vec<u8>>, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "{{{} credential id(s), redacted}}", ids.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8] = b"yubikey-5-credential-id";

    #[test]
    fn authenticating_accepts_only_an_allowed_key() {
        let ceremony = SecurityKeyCeremony::Authenticate {
            credential_ids: vec![KEY.to_vec()],
        };
        assert!(ceremony.accepts(KEY));
        assert!(!ceremony.accepts(b"another-key"));
        assert!(!ceremony.accepts(b""));
    }

    #[test]
    fn registering_accepts_a_new_key_and_refuses_an_enrolled_or_empty_one() {
        let ceremony = SecurityKeyCeremony::Register {
            user_id: vec![9; 32],
            user_name: "alice".into(),
            exclude_credential_ids: vec![KEY.to_vec()],
        };
        assert!(ceremony.accepts(b"a-new-credential"));
        assert!(!ceremony.accepts(KEY), "an enrolled key was accepted again");
        assert!(
            !ceremony.accepts(b""),
            "an empty credential id was accepted"
        );
    }

    #[test]
    fn a_ceremony_never_prints_a_credential_id_and_samples_the_user_handle() {
        let printed = format!(
            "{:?}",
            SecurityKeyCeremony::Register {
                user_id: vec![0x5a; 32],
                user_name: "alice".into(),
                exclude_credential_ids: vec![KEY.to_vec()],
            }
        );
        assert!(printed.contains("BytesLike(len: 32"), "{printed}");
        let leading = format!("{:?}", &KEY[..5]);
        assert!(
            !printed.contains(leading.trim_matches(|c| c == '[' || c == ']')),
            "{printed}"
        );
        assert!(
            printed.contains("1 credential id(s), redacted"),
            "{printed}"
        );
    }
}
