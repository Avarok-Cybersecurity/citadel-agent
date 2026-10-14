//! The IO shell around `CollectorCore`: the lock, the OS randomness for batch salts, and
//! a flag the tracing layer reads without locking.
//!
//! Nothing here touches the disk. A ring lives only in this process's memory.

use super::config::{RingConfig, TelemetryError};
use super::core::CollectorCore;
use super::entry::{CapturedEvent, TelemetryEntry};
use super::redact::{KnownValue, Salt};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Debug)]
pub struct TelemetryCollector {
    core: Mutex<CollectorCore>,
    /// Mirrors `core.is_capturing()` after every change, so the layer's idle path
    /// takes no lock.
    capturing: AtomicBool,
}

impl TelemetryCollector {
    pub fn new(config: RingConfig) -> Arc<Self> {
        Arc::new(Self {
            core: Mutex::new(CollectorCore::new(config)),
            capturing: AtomicBool::new(false),
        })
    }

    pub fn register_session(&self, cid: u64, identity: Vec<KnownValue>) {
        self.core.lock().register_session(cid, identity);
    }

    pub fn forget_session(&self, cid: u64) {
        self.update(|core| {
            core.forget_session(cid);
            Ok(())
        })
        .expect("forgetting a session cannot fail");
    }

    pub fn learn_peer(
        &self,
        session_cid: u64,
        peer_cid: u64,
        identity: Vec<KnownValue>,
    ) -> Result<(), TelemetryError> {
        self.core.lock().learn_peer(session_cid, peer_cid, identity)
    }

    pub fn opt_in(&self, cid: u64) -> Result<(), TelemetryError> {
        let salt = fresh_salt()?;
        self.update(|core| core.opt_in(cid, salt))
    }

    pub fn opt_out(&self, cid: u64) -> Result<(), TelemetryError> {
        self.update(|core| core.opt_out(cid))
    }

    /// Everything `cid` captured since the last drain, redacted under one salt. The next
    /// batch gets a new salt, so placeholders cannot be joined across uploads.
    pub fn drain(&self, cid: u64) -> Result<Vec<TelemetryEntry>, TelemetryError> {
        let salt = fresh_salt()?;
        self.core.lock().drain(cid, salt)
    }

    pub fn opted_in_sessions(&self) -> Vec<u64> {
        self.core.lock().opted_in()
    }

    pub fn is_capturing(&self) -> bool {
        self.capturing.load(Ordering::Relaxed)
    }

    pub(super) fn capture(&self, event: &CapturedEvent<'_>) {
        self.core.lock().capture(event);
    }

    fn update(
        &self,
        change: impl FnOnce(&mut CollectorCore) -> Result<(), TelemetryError>,
    ) -> Result<(), TelemetryError> {
        let mut core = self.core.lock();
        let result = change(&mut core);
        self.capturing.store(core.is_capturing(), Ordering::Relaxed);
        result
    }
}

fn fresh_salt() -> Result<Salt, TelemetryError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|err| TelemetryError::Randomness(err.to_string()))?;
    Ok(Salt::from_bytes(bytes))
}
