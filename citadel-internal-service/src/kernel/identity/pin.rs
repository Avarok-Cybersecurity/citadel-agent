//! What an identity account pinned at registration (`server-identity-challenge.md` §3): its
//! handle, the server's canonical domain and key, and the identity that registered it. Kept in
//! the account's own byte map beside `_AGENT_SERVER_HOST` (server_host.rs), so it is persisted
//! with the account and deleted with it, and no `LocalDB*` request can reach it.
//!
//! Written at registration and, for a `Pinned` registration, once more when the first challenge
//! proves the key behind the fingerprint. Never otherwise: a different key is a warning, not a
//! re-pin.

use citadel_identity::{canonical_domain, fingerprint, CanonicalDomain, MlDsa65Pk};
use citadel_internal_service_types::AccountIdentity;
use citadel_sdk::prelude::{NodeRemote, Ratchet};
use serde::{Deserialize, Serialize};

const KEY: &str = "_AGENT_IDENTITY_PIN";
const SUB_KEY: &str = "pin";
const SELF_PEER_CID: u64 = 0;
const PIN_VERSION: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pin {
    pub(crate) handle: String,
    pub(crate) domain: CanonicalDomain,
    pub(crate) server_fp: String,
    /// Known from `/.well-known/citadel`, or from the first matching challenge of a `Pinned`
    /// registration; `None` until then.
    pub(crate) server_pk: Option<MlDsa65Pk>,
    pub(crate) issued_at_s: Option<u64>,
    pub(crate) id_fp: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    v: u8,
    handle: String,
    canonical_domain: String,
    server_fp: String,
    server_pk: Option<String>,
    issued_at_s: Option<u64>,
    id_fp: String,
}

impl Pin {
    /// Verified once the key behind the fingerprint is known and matched it.
    pub(crate) fn verified(&self) -> bool {
        self.server_pk.is_some()
    }

    /// Registered from a fetched `/.well-known/citadel`, which dates it; a `Pinned` registration
    /// has no document and never records a date.
    pub(crate) fn discovered(&self) -> bool {
        self.issued_at_s.is_some()
    }

    pub(crate) fn wire(&self) -> AccountIdentity {
        AccountIdentity {
            handle: self.handle.clone(),
            canonical_domain: self.domain.to_string(),
            id_fp: self.id_fp.clone(),
            server_fp: self.server_fp.clone(),
            verified: self.verified(),
        }
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let wire = Wire {
            v: PIN_VERSION,
            handle: self.handle.clone(),
            canonical_domain: self.domain.to_string(),
            server_fp: self.server_fp.clone(),
            server_pk: self.server_pk.as_ref().map(MlDsa65Pk::to_base64url),
            issued_at_s: self.issued_at_s,
            id_fp: self.id_fp.clone(),
        };
        serde_json::to_vec(&wire).unwrap_or_else(|_| unreachable!("the pin is plain data"))
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, String> {
        let wire: Wire = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if wire.v != PIN_VERSION {
            return Err(format!("pin version {} is not {PIN_VERSION}", wire.v));
        }
        let domain = canonical_domain(&wire.canonical_domain).map_err(|e| e.to_string())?;
        if domain.as_str() != wire.canonical_domain {
            return Err(format!("{:?} is not canonical", wire.canonical_domain));
        }
        let server_pk = match wire.server_pk {
            Some(pk) => Some(MlDsa65Pk::from_base64url(&pk).map_err(|e| e.to_string())?),
            None => None,
        };
        if let Some(pk) = &server_pk {
            if fingerprint(pk) != wire.server_fp {
                return Err("the pinned key is not the pinned fingerprint's".to_string());
            }
        }
        Ok(Self {
            handle: wire.handle,
            domain,
            server_fp: wire.server_fp,
            server_pk,
            issued_at_s: wire.issued_at_s,
            id_fp: wire.id_fp,
        })
    }
}

/// A typed server fingerprint (`abcd-efgh-ijkl-mnop`, dashes optional, any case) in the form
/// `fingerprint` gives it.
pub(crate) fn normalise_fingerprint(typed: &str) -> Result<String, String> {
    let fp: String = typed
        .chars()
        .filter(|c| *c != '-')
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let base32 = fp
        .bytes()
        .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b));
    if fp.len() != 16 || !base32 {
        return Err(format!(
            "{typed:?} is not a server fingerprint (16 base32 characters)"
        ));
    }
    Ok(fp)
}

pub(crate) async fn store<R: Ratchet>(
    remote: &NodeRemote<R>,
    cid: u64,
    pin: &Pin,
) -> Result<(), String> {
    remote
        .account_manager()
        .get_persistence_handler()
        .store_byte_map_value(cid, SELF_PEER_CID, KEY, SUB_KEY, pin.encode())
        .await
        .map(|_previous| ())
        .map_err(|err| err.into_string())
}

/// Account `cid`'s pin; `None` for an account registered with a typed password.
pub(crate) async fn load<R: Ratchet>(
    remote: &NodeRemote<R>,
    cid: u64,
) -> Result<Option<Pin>, String> {
    let stored = remote
        .account_manager()
        .get_persistence_handler()
        .get_byte_map_value(cid, SELF_PEER_CID, KEY, SUB_KEY)
        .await
        .map_err(|err| err.into_string())?;
    stored
        .map(|bytes| Pin::decode(&bytes).map_err(|err| format!("account {cid}'s pin: {err}")))
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use citadel_identity::ServerKey;

    fn pin(server_pk: Option<MlDsa65Pk>) -> Pin {
        let key = ServerKey::from_seed(&[11; 32]);
        Pin {
            handle: "alice".into(),
            domain: canonical_domain("acme.example").unwrap(),
            server_fp: fingerprint(key.public_key()),
            server_pk,
            issued_at_s: Some(1_794_700_000),
            id_fp: "3foriftckvpoy2rh".into(),
        }
    }

    #[test]
    fn a_pin_round_trips_verified_or_not() {
        let key = ServerKey::from_seed(&[11; 32]);
        for p in [pin(None), pin(Some(key.public_key().clone()))] {
            assert_eq!(Pin::decode(&p.encode()).unwrap(), p);
        }
        assert!(!pin(None).verified());
        assert!(pin(Some(key.public_key().clone())).wire().verified);
    }

    #[test]
    fn a_key_that_is_not_the_fingerprints_is_refused() {
        let other = ServerKey::from_seed(&[12; 32]);
        let bad = pin(Some(other.public_key().clone()));
        assert!(Pin::decode(&bad.encode()).is_err());
    }

    #[test]
    fn typed_fingerprints_normalise() {
        assert_eq!(
            normalise_fingerprint("56NA-tocv-ZNOI-bcd5").unwrap(),
            "56natocvznoibcd5"
        );
        assert!(normalise_fingerprint("56natocvznoibcd").is_err());
        assert!(normalise_fingerprint("56natocvznoibcd1").is_err());
    }
}
