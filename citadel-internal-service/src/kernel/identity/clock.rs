//! The identity's monotonic time, as a port: the agent reads the runtime's clock, and a test
//! states how much time has passed instead of depending on how fast the box runs Argon2id.

use tokio::time::Instant;

pub trait IdentityClock: Send + Sync {
    fn now(&self) -> Instant;
}

/// The runtime's monotonic clock: what the agent runs.
pub struct MonotonicClock;

impl IdentityClock for MonotonicClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}
