//! The unsealed identity, held for a bounded idle window and dropped (its keys wiped by
//! `Identity`'s own drop) on lock or once the window passes. The seed itself is never held:
//! `Identity` keeps only the expanded signing key and the password key.

use citadel_identity::Identity;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

pub(crate) struct Unlocked {
    idle: Duration,
    held: Mutex<Option<(Arc<Identity>, Instant)>>,
}

impl Unlocked {
    pub(crate) fn new(idle: Duration) -> Self {
        Self {
            idle,
            held: Mutex::default(),
        }
    }

    pub(crate) fn hold(&self, identity: Identity) -> Arc<Identity> {
        let identity = Arc::new(identity);
        *self.held.lock() = Some((identity.clone(), Instant::now()));
        identity
    }

    pub(crate) fn lock(&self) {
        *self.held.lock() = None;
    }

    /// The identity, if unlocked and used within the idle window; a use restarts the window.
    pub(crate) fn current(&self) -> Option<Arc<Identity>> {
        let mut held = self.held.lock();
        match held.as_mut() {
            Some((identity, at)) if at.elapsed() < self.idle => {
                *at = Instant::now();
                Some(identity.clone())
            }
            Some(_) => {
                *held = None;
                None
            }
            None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn the_identity_is_forgotten_after_the_idle_window_or_on_lock() {
        let unlocked = Unlocked::new(Duration::from_secs(60));
        unlocked.hold(Identity::from_seed(&[1; 32]));
        tokio::time::advance(Duration::from_secs(59)).await;
        assert!(unlocked.current().is_some(), "a use restarts the window");
        tokio::time::advance(Duration::from_secs(59)).await;
        assert!(unlocked.current().is_some());
        tokio::time::advance(Duration::from_secs(61)).await;
        assert!(unlocked.current().is_none());
        unlocked.hold(Identity::from_seed(&[1; 32]));
        unlocked.lock();
        assert!(unlocked.current().is_none());
    }
}
