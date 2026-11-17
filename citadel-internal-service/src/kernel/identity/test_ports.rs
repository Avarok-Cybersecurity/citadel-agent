//! Ports for the identity's unit tests: an in-memory store, a fetch that serves fixed
//! documents, a codec that frames nothing, and a clock that moves only when told. Only the store is exercised by most tests; the
//! fetch and codec are required by `IdentityConfig` (no defaults).

use super::{
    IdentityClock, IdentityConfig, IdentityHost, MemoryIdentityStore, ServerIdentityAnswer,
    ServerIdentityCodec, WellKnownFetch,
};
use async_trait::async_trait;
use citadel_identity::{CanonicalDomain, NONCE_LEN};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

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

/// Time that passes only by `advance`, so "inside the interval" is the test's statement.
pub(crate) struct ManualClock(Mutex<Instant>);

impl ManualClock {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self(Mutex::new(Instant::now())))
    }

    pub(crate) fn advance(&self, by: Duration) {
        *self.0.lock() += by;
    }
}

impl IdentityClock for ManualClock {
    fn now(&self) -> Instant {
        *self.0.lock()
    }
}

pub(crate) fn host() -> IdentityHost {
    host_with(SHIPPED_SIGN_LIMITS)
}

/// The protocol's rate, so a test sees the agent's own rate limits unless it states others.
pub(crate) const SHIPPED_SIGN_LIMITS: super::SignLimits = super::SignLimits::PROTOCOL;

/// A host with `limits`, for a test that needs another rate than the shipped one.
pub(crate) fn host_with(limits: super::SignLimits) -> IdentityHost {
    host_and_clock(limits).0
}

/// A host with `limits` and the clock it measures them on.
pub(crate) fn host_and_clock(limits: super::SignLimits) -> (IdentityHost, Arc<ManualClock>) {
    let clock = ManualClock::new();
    let host = IdentityHost::new(config(limits, Arc::new(NoFetch), clock.clone()));
    (host, clock)
}

/// The unit tests' configuration: an in-memory store, no codec, `well_known` and `clock`.
pub(crate) fn config(
    limits: super::SignLimits,
    well_known: Arc<dyn WellKnownFetch>,
    clock: Arc<dyn IdentityClock>,
) -> IdentityConfig {
    IdentityConfig {
        store: Arc::new(MemoryIdentityStore::default()),
        well_known,
        codec: Arc::new(NoCodec),
        // Long enough that no test sees it pass.
        unlock_idle: Duration::from_secs(3600),
        sign_limits: limits,
        clock,
    }
}
