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
    IdentityHost::new(IdentityConfig {
        store: Arc::new(MemoryIdentityStore::default()),
        well_known: Arc::new(NoFetch),
        codec: Arc::new(NoCodec),
        // Long enough that no test sees it pass.
        unlock_idle: Duration::from_secs(3600),
        challenge_timeout: Duration::from_secs(5),
    })
}
