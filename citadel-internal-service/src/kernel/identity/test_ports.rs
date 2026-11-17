//! Ports for the identity's unit tests: an in-memory store, a fetch that serves fixed
//! documents, and a codec that frames nothing. Only the store is exercised by most tests; the
//! fetch and codec are required by `IdentityConfig` (no defaults).

use super::{
    IdentityConfig, IdentityHost, MemoryIdentityStore, ServerIdentityAnswer, ServerIdentityCodec,
    WellKnownFetch,
};
use async_trait::async_trait;
use citadel_identity::{CanonicalDomain, NONCE_LEN};
use std::sync::Arc;
use std::time::Duration;

pub(crate) struct NoFetch;

#[async_trait]
impl WellKnownFetch for NoFetch {
    async fn fetch(&self, domain: &CanonicalDomain) -> Result<Vec<u8>, String> {
        Err(format!("no network in unit tests ({domain})"))
    }
}

/// Serves one fixed document for every domain, as a fetched `/.well-known/citadel`.
pub(crate) struct ServeDocument(pub(crate) &'static [u8]);

#[async_trait]
impl WellKnownFetch for ServeDocument {
    async fn fetch(&self, _domain: &CanonicalDomain) -> Result<Vec<u8>, String> {
        Ok(self.0.to_vec())
    }
}

pub(crate) struct NoCodec;

impl ServerIdentityCodec for NoCodec {
    fn challenge(&self, _nonce: &[u8; NONCE_LEN]) -> Result<Vec<u8>, String> {
        Err("no server in unit tests".into())
    }

    fn answer(&self, _frame: &[u8]) -> Option<ServerIdentityAnswer> {
        None
    }
}

pub(crate) fn host() -> IdentityHost {
    host_with(SHIPPED_SIGN_LIMITS)
}

/// The shipped `[identity]` values (`sign-in-with-citadel.md` §8), so a test sees the agent's
/// own rate limits unless it states others.
pub(crate) const SHIPPED_SIGN_LIMITS: super::SignLimits = super::SignLimits {
    min_interval: Duration::from_secs(2),
    wrong_password_limit: 5,
    lockout: Duration::from_secs(60),
};

/// A host with `limits`, for a test that needs another rate than the shipped one.
pub(crate) fn host_with(limits: super::SignLimits) -> IdentityHost {
    IdentityHost::new(config(limits, Arc::new(NoFetch)))
}

/// The unit tests' configuration: an in-memory store, no codec, and `well_known`.
pub(crate) fn config(
    limits: super::SignLimits,
    well_known: Arc<dyn WellKnownFetch>,
) -> IdentityConfig {
    IdentityConfig {
        store: Arc::new(MemoryIdentityStore::default()),
        well_known,
        codec: Arc::new(NoCodec),
        // Long enough that no test sees it pass.
        unlock_idle: Duration::from_secs(3600),
        sign_limits: limits,
    }
}
