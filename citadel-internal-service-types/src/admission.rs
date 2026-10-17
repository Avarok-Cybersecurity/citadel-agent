//! The check a server's admission refusal asks the window to show (the SDK's
//! `AdmissionChallenge`), so a server reached only through the agent -- a self-hosted one, with
//! no HTTP discovery -- can still be signed in to.
//!
//! PUBLIC by contract: a Turnstile site key is handed to every visitor, and the identity is the
//! value the token is bound to (Turnstile's `cData`). It never holds a secret.

use serde::{Deserialize, Serialize};

#[cfg(feature = "typescript")]
use ts_rs::TS;

/// Who issues the token the check produces.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum AdmissionProviderWire {
    Turnstile,
}

/// The widget to render: its provider, PUBLIC site key and the `cData` to bind the token to.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct AdmissionChallengeWire {
    pub provider: AdmissionProviderWire,
    pub site_key: String,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub identity: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_challenge_crosses_the_wire_in_snake_case_and_identity_is_optional() {
        let challenge = AdmissionChallengeWire {
            provider: AdmissionProviderWire::Turnstile,
            site_key: "0x4AAA".into(),
            identity: Some("acme".into()),
        };
        let json = serde_json::to_string(&challenge).unwrap();
        assert_eq!(
            json,
            r#"{"provider":"turnstile","site_key":"0x4AAA","identity":"acme"}"#
        );
        let bare: AdmissionChallengeWire =
            serde_json::from_str(r#"{"provider":"turnstile","site_key":"k"}"#).unwrap();
        assert_eq!(bare.identity, None);
    }
}
