//! What the core remembers about one peer, and the backoff arithmetic.

use super::super::policy::Backoff;
use super::super::types::{Cid, Millis};
use citadel_internal_service_types::P2pPathReport;
use std::time::Duration;

/// One retried action: when it may next run, and how many times it has run since the last
/// stable route.
#[derive(Debug, Clone, Copy)]
pub(super) struct Retry {
    pub attempts: u32,
    pub next_at: Millis,
    pub in_flight: bool,
}

impl Retry {
    pub fn new(now: Millis) -> Self {
        Self {
            attempts: 0,
            next_at: now,
            in_flight: false,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct Peer {
    pub connected: bool,
    pub path: Option<P2pPathReport>,
    /// When the current connection came up.
    pub since: Option<Millis>,
    /// When a connected peer was last lost; cleared when it is back.
    pub lost_at: Option<Millis>,
    pub backlog: u32,
    pub interest_until: Option<Millis>,
    pub dial: Retry,
    pub upgrade: Retry,
    pub dialing_reported: bool,
    pub degraded_reported: bool,
}

impl Peer {
    pub fn new(now: Millis) -> Self {
        Self {
            connected: false,
            path: None,
            since: None,
            lost_at: None,
            backlog: 0,
            interest_until: None,
            dial: Retry::new(now),
            upgrade: Retry::new(now),
            dialing_reported: false,
            degraded_reported: false,
        }
    }

    pub fn has_interest(&self, now: Millis) -> bool {
        self.interest_until.is_some_and(|until| until > now)
    }

    /// Queued messages or an open window: the reasons to heal a path, not just reach it.
    pub fn in_use(&self, now: Millis) -> bool {
        self.backlog > 0 || self.has_interest(now)
    }

    pub fn wanted(&self, now: Millis, redial_window: Duration) -> bool {
        self.in_use(now)
            || self
                .lost_at
                .is_some_and(|lost| now.since(lost) < redial_window)
    }

    pub fn on_relay(&self) -> bool {
        self.connected && self.path == Some(P2pPathReport::ServerRelay)
    }
}

/// splitmix64: one 64-bit state, no dependencies, the same sequence for the same seed.
#[derive(Debug, Clone, Copy)]
pub(super) struct Rng(u64);

impl Rng {
    pub fn seeded(seed: u64, cid: Cid) -> Self {
        Self(seed ^ cid)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// `backoff` doubled `attempts` times and capped, less up to `jitter_permille`/1000 of
    /// itself: jitter only shortens, so the cap holds.
    pub fn delay(&mut self, backoff: Backoff, attempts: u32, jitter_permille: u32) -> Duration {
        let doubled = backoff
            .initial
            .saturating_mul(1u32.checked_shl(attempts).unwrap_or(u32::MAX));
        let capped = doubled.min(backoff.max);
        let millis = u64::try_from(capped.as_millis()).unwrap_or(u64::MAX);
        let span = millis.saturating_mul(u64::from(jitter_permille)) / 1000;
        let cut = if span == 0 { 0 } else { self.next() % span };
        Duration::from_millis(millis - cut)
    }
}
