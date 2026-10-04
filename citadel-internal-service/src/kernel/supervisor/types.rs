//! The vocabulary of the pure core: what goes in, what comes out. Plain data, no I/O.

use citadel_internal_service_types::{P2pPathReport, SupervisorState};
use std::time::Duration;

pub type Cid = u64;

/// A point on the supervisor's clock, in milliseconds. The core never reads a clock: every
/// input carries the time it happened, so a test advances time by constructing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Millis(pub u64);

impl Millis {
    pub fn after(self, wait: Duration) -> Millis {
        let wait = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX);
        Millis(self.0.saturating_add(wait))
    }

    pub fn since(self, earlier: Millis) -> Duration {
        Duration::from_millis(self.0.saturating_sub(earlier.0))
    }
}

/// Where the account's server link stands, as `kernel/reconnect` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkStatus {
    Up,
    Reconnecting,
    /// The session is over; the core goes quiet for good.
    Ended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeOutcome {
    Ok {
        rtt: Duration,
    },
    /// The server did not answer in time: evidence the path is dead.
    Timeout,
    /// The probe could not be made; says nothing about the path.
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialOutcome {
    Connected,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    Tick {
        now: Millis,
    },
    /// The set of interfaces or addresses changed.
    NetworkChanged {
        now: Millis,
    },
    ServerProbe {
        now: Millis,
        outcome: ProbeOutcome,
    },
    ServerLink {
        now: Millis,
        state: LinkStatus,
    },
    /// `ForceReconnect` could not end the link; it is as it was.
    ForceFailed {
        now: Millis,
    },
    PeerLost {
        now: Millis,
        peer: Cid,
    },
    /// The user ended the connection on purpose: it is not to be redialled, and a window
    /// that had it open no longer asks for it.
    PeerReleased {
        now: Millis,
        peer: Cid,
    },
    PeerUp {
        now: Millis,
        peer: Cid,
        path: P2pPathReport,
    },
    PeerPath {
        now: Millis,
        peer: Cid,
        path: P2pPathReport,
    },
    DialResult {
        now: Millis,
        peer: Cid,
        outcome: DialOutcome,
    },
    /// Messages still queued for `peer` in the account's ILM.
    Backlog {
        peer: Cid,
        pending: u32,
    },
    /// A window has `peer` open (a chat or a call) until `until`.
    Interest {
        peer: Cid,
        until: Millis,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// A fast liveness check of the server link.
    ProbeServer,
    /// Hand the link to `kernel/reconnect` now, instead of waiting out the keep-alive.
    ForceReconnect,
    /// QUIC endpoints rebind to the new local address.
    RebindTransports,
    DialPeer {
        peer: Cid,
    },
    /// Re-arm the SDK's path campaign for `peer`.
    UpgradePath {
        peer: Cid,
        restore_udp: bool,
    },
    Report(SupervisorEvent),
}

/// Why the supervisor started healing the server link. For logs; windows are told only that
/// it is healing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealCause {
    /// The link dropped and `kernel/reconnect` is bringing it back.
    LinkLost,
    /// The server stopped answering probes.
    ProbesMissed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupervisorEvent {
    Healing { cause: HealCause },
    Healed,
    PeerDialing { peer_cid: Cid },
    PeerRestored { peer_cid: Cid, path: P2pPathReport },
    Degraded { peer_cid: Cid },
}

impl SupervisorEvent {
    /// What windows are told: the peer it is about, if any, and the state.
    pub fn wire(self) -> (Option<Cid>, SupervisorState) {
        match self {
            Self::Healing { .. } => (None, SupervisorState::Healing),
            Self::Healed => (None, SupervisorState::Healed),
            Self::PeerDialing { peer_cid } => (Some(peer_cid), SupervisorState::Healing),
            Self::PeerRestored { peer_cid, .. } => (Some(peer_cid), SupervisorState::Healed),
            Self::Degraded { peer_cid } => (Some(peer_cid), SupervisorState::Degraded),
        }
    }
}
