//! Every number the supervisor decides by. Built explicitly by the binary: there is no
//! `Default`, so a caller that forgets a field does not compile and nothing is guessed.

use std::time::Duration;

/// Doubling from `initial` up to `max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    pub initial: Duration,
    pub max: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupervisorPolicy {
    /// How often the server link is probed while it is quiet.
    pub probe_interval: Duration,
    /// How long one probe may take before it counts as missed.
    pub probe_timeout: Duration,
    /// Consecutive missed probes that end the link.
    pub missed_probes: u32,
    pub dial_backoff: Backoff,
    pub upgrade_backoff: Backoff,
    /// How long a route must hold before its backoff resets.
    pub stable_after: Duration,
    /// How long after a drop an account's peer is redialled with nothing queued for it and
    /// no window open for it.
    pub redial_window: Duration,
    /// The longest a window's interest in a peer holds without being repeated, whatever it
    /// asked for: a window that is gone cannot pin a peer connected.
    pub interest_ceiling: Duration,
    /// How often the ILM queue depth is read.
    pub backlog_poll: Duration,
    /// Seeds the backoff jitter, so peers do not redial in lockstep.
    pub jitter_seed: u64,
    /// Fraction of a backoff (in thousandths) that jitter may remove.
    pub jitter_permille: u32,
    /// Whether this supervisor dials peers. When it does not, windows keep dialling, and the
    /// agent does not declare `supervises_p2p`.
    pub dial_peers: bool,
}

/// What the shipped agent runs. A probe every 15 s with 5 s to answer and two misses means a
/// dead path is noticed within 25 s, against the protocol keep-alive's 45 minutes.
pub const AGENT_SUPERVISOR: SupervisorPolicy = SupervisorPolicy {
    probe_interval: Duration::from_secs(15),
    probe_timeout: Duration::from_secs(5),
    missed_probes: 2,
    dial_backoff: Backoff {
        initial: Duration::from_secs(1),
        max: Duration::from_secs(30),
    },
    upgrade_backoff: Backoff {
        initial: Duration::from_secs(5),
        max: Duration::from_secs(300),
    },
    stable_after: Duration::from_secs(60),
    redial_window: Duration::from_secs(600),
    interest_ceiling: Duration::from_secs(120),
    backlog_poll: Duration::from_secs(2),
    jitter_seed: 0x9E37_79B9_7F4A_7C15,
    jitter_permille: 250,
    dial_peers: true,
};
