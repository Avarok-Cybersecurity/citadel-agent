//! How the agent answers a peer's PeerConnect for an account it hosts. Pure: the
//! facts are read by the caller (see the module above), and decided here.
//!
//! The rules are the web UI's (`p2p-auto-connect-service/incoming-connect.ts`),
//! in its order: a paused contact is declined, an unreadable pause record gets
//! no answer, an offer below the chat's level is declined, anything else from a
//! registered peer is accepted.

use citadel_internal_service_types::SecurityLevel;

/// The pause record the UI keeps for (account, peer) in the agent's LocalDB
/// (`p2p-pause/pause-rules.ts`): set on pause, deleted on resume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PauseRecord {
    Absent,
    Paused,
    /// A read error, or a value that is not the marker: nobody knows.
    Unreadable,
}

/// The value the UI writes for a pause (`PAUSED_MARKER`).
pub(crate) const PAUSED_MARKER: &[u8] = b"paused";

impl PauseRecord {
    /// `stored` is the LocalDB read: `Ok(None)` is an absent key.
    pub(crate) fn from_read(stored: &Result<Option<Vec<u8>>, String>) -> Self {
        match stored {
            Ok(None) => PauseRecord::Absent,
            Ok(Some(value)) if value == PAUSED_MARKER => PauseRecord::Paused,
            Ok(Some(_)) | Err(_) => PauseRecord::Unreadable,
        }
    }
}

/// What the agent does with an offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentAnswer {
    Accept,
    Decline,
    /// The agent handles the offer by not answering it: the initiator retries.
    NoAnswer,
    /// Not the agent's to answer; a window may (as before the agent answered).
    LeaveToWindows,
}

impl AgentAnswer {
    /// Whether windows are told the agent has this offer (they then send nothing).
    pub(crate) fn agent_has_it(self) -> bool {
        self != AgentAnswer::LeaveToWindows
    }
}

/// What the caller read about one offer to an account the agent hosts.
#[derive(Debug, Clone, Copy)]
pub(crate) struct OfferFacts {
    /// The peer is registered with the account; `None` when the registry was unreadable.
    pub registered: Option<bool>,
    pub pause: PauseRecord,
    pub offered: SecurityLevel,
    /// The chat's minimum level; `None` when the account's preferences were unreadable.
    pub minimum: Option<SecurityLevel>,
}

pub(crate) fn decide(facts: OfferFacts) -> AgentAnswer {
    if facts.registered != Some(true) {
        return AgentAnswer::LeaveToWindows;
    }
    match facts.pause {
        PauseRecord::Paused => return AgentAnswer::Decline,
        PauseRecord::Unreadable => return AgentAnswer::NoAnswer,
        PauseRecord::Absent => {}
    }
    match facts.minimum {
        // The UI's read of an unreadable level throws, and nothing answers.
        None => AgentAnswer::NoAnswer,
        Some(minimum) if facts.offered.value() < minimum.value() => AgentAnswer::Decline,
        Some(_) => AgentAnswer::Accept,
    }
}
