//! The agent's portable identity (`ACCOUNT-IDENTITY-V2-PORTABLE.md` rev b §4): one seed, sealed
//! at rest, unlocked for a bounded idle window; every server password and binding derives from
//! it through `citadel-identity`. The UI never sees the seed.
//!
//! I/O sits behind ports the binary supplies (`IdentityConfig`): the record store, the
//! `/.well-known/citadel` fetch and the workspace protocol's challenge codec. An agent built
//! without `with_identity` refuses every identity request as `NotConfigured`.

pub(crate) mod challenge;
pub(crate) mod discovery;
mod lifecycle;
pub(crate) mod pin;
pub(crate) mod plan;
pub(crate) mod record;
pub(crate) mod seal;
mod sign_challenge;
mod sign_gate;
pub(crate) mod store;
#[cfg(test)]
pub(crate) mod test_ports;
mod unlocked;

pub use challenge::{ServerIdentityAnswer, ServerIdentityCodec};
pub use discovery::{HttpsWellKnownFetch, WellKnownFetch, MAX_WELL_KNOWN_BYTES};
pub(crate) use sign_challenge::SignRequest;
pub use sign_gate::SignLimits;
pub use store::{FsIdentityStore, IdentityStore, MemoryIdentityStore};

use challenge::ChallengeWaiters;
use citadel_internal_service_types::AccountIdentity;
pub(crate) use lifecycle::Refused;
use parking_lot::{Mutex, RwLock};
use sign_gate::SignGate;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use unlocked::Unlocked;

/// Everything the identity needs from the binary. No field has a default.
pub struct IdentityConfig {
    pub store: Arc<dyn IdentityStore>,
    pub well_known: Arc<dyn WellKnownFetch>,
    pub codec: Arc<dyn ServerIdentityCodec>,
    /// How long the unsealed identity is kept after its last use.
    pub unlock_idle: Duration,
    /// How long a connect waits for the server's challenge answer before refusing it.
    pub challenge_timeout: Duration,
    /// How often `IdentitySignChallenge` may run (`[identity]` in the agent's config).
    pub sign_limits: SignLimits,
}

pub(crate) struct IdentityHost {
    pub(crate) store: Arc<dyn IdentityStore>,
    pub(crate) well_known: Arc<dyn WellKnownFetch>,
    pub(crate) waiters: Arc<ChallengeWaiters>,
    pub(crate) challenge_timeout: Duration,
    pub(crate) unlocked: Unlocked,
    sign_gate: Mutex<SignGate>,
    /// What each connected identity session pinned, for `GetSessions` (polled, so not read
    /// from the account store each time). Filled by a verified connect.
    sessions: RwLock<HashMap<u64, AccountIdentity>>,
}

impl IdentityHost {
    pub(crate) fn new(config: IdentityConfig) -> Self {
        Self {
            store: config.store,
            well_known: config.well_known,
            waiters: Arc::new(ChallengeWaiters::new(config.codec)),
            challenge_timeout: config.challenge_timeout,
            unlocked: Unlocked::new(config.unlock_idle),
            sign_gate: Mutex::new(SignGate::new(config.sign_limits)),
            sessions: RwLock::default(),
        }
    }

    pub(crate) fn session_identity(&self, cid: u64) -> Option<AccountIdentity> {
        self.sessions.read().get(&cid).cloned()
    }

    pub(crate) fn record_session(&self, cid: u64, identity: AccountIdentity) {
        self.sessions.write().insert(cid, identity);
    }

    pub(crate) fn forget_session(&self, cid: u64) {
        self.sessions.write().remove(&cid);
        self.waiters.forget(cid);
    }
}
