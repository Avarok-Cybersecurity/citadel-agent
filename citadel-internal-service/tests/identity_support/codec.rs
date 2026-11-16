//! The challenge codec and the (unused) well-known fetch the identity tests' agent is built
//! with. The JSON is the workspace protocol's for the two messages (see `mod.rs`).

use citadel_identity::NONCE_LEN;
use citadel_internal_service::kernel::identity::{
    ServerIdentityAnswer, ServerIdentityCodec, WellKnownFetch,
};
use serde_json::{json, Value};

pub struct JsonCodec;

impl ServerIdentityCodec for JsonCodec {
    fn challenge(&self, nonce: &[u8; NONCE_LEN]) -> Result<Vec<u8>, String> {
        serde_json::to_vec(&json!({"Request": {"ServerIdentityChallenge": {"nonce": nonce}}}))
            .map_err(|e| e.to_string())
    }

    fn answer(&self, frame: &[u8]) -> Option<ServerIdentityAnswer> {
        let value: Value = serde_json::from_slice(frame).ok()?;
        let answer = value.get("Response")?.get("ServerIdentity")?;
        Some(
            serde_json::from_value::<Answer>(answer.clone())
                .ok()?
                .into(),
        )
    }
}

#[derive(serde::Deserialize)]
struct Answer {
    domain: String,
    pk: Vec<u8>,
    sig: Vec<u8>,
}

impl From<Answer> for ServerIdentityAnswer {
    fn from(a: Answer) -> Self {
        ServerIdentityAnswer {
            domain: a.domain,
            pk: a.pk,
            sig: a.sig,
        }
    }
}

/// Every server in these tests is dialled as `host:port`, so the `Pinned` discovery is used and
/// nothing is fetched.
pub struct NoFetch;

#[async_trait::async_trait]
impl WellKnownFetch for NoFetch {
    async fn fetch(&self, domain: &citadel_identity::CanonicalDomain) -> Result<Vec<u8>, String> {
        Err(format!(
            "the identity tests serve no /.well-known/citadel ({domain})"
        ))
    }
}
