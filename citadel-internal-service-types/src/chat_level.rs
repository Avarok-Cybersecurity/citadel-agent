//! A chat's encryption level, as the web UI's chat settings name it
//! (`lib/p2p/chat-advanced-settings.ts`, `ChatSecurityLevel`), and the per-peer
//! minimum an account pushes so the agent can answer connects with no window.

use crate::SecurityLevel;
use serde::{Deserialize, Serialize};

#[cfg(feature = "typescript")]
use ts_rs::TS;

/// The levels a chat can promise: the four the WASM message path knows.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum ChatSecurityLevel {
    Standard,
    Reinforced,
    High,
    Extreme,
}

impl ChatSecurityLevel {
    /// The SDK's level of the same name; its `value()` is what offers are ranked by.
    pub fn sdk(self) -> SecurityLevel {
        match self {
            ChatSecurityLevel::Standard => SecurityLevel::Standard,
            ChatSecurityLevel::Reinforced => SecurityLevel::Reinforced,
            ChatSecurityLevel::High => SecurityLevel::High,
            ChatSecurityLevel::Extreme => SecurityLevel::Extreme,
        }
    }

    /// Whether a connection offered at `offered` satisfies a chat with this minimum.
    pub fn admits(self, offered: SecurityLevel) -> bool {
        offered.value() >= self.sdk().value()
    }
}

/// One chat's minimum: offers from `peer_cid` below `level` are declined.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct PeerSecurityMinimum {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub peer_cid: u64,
    pub level: ChatSecurityLevel,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minimum_admits_its_own_level_and_above_only() {
        assert!(ChatSecurityLevel::Standard.admits(SecurityLevel::Standard));
        assert!(ChatSecurityLevel::High.admits(SecurityLevel::High));
        assert!(ChatSecurityLevel::High.admits(SecurityLevel::Extreme));
        assert!(!ChatSecurityLevel::High.admits(SecurityLevel::Reinforced));
        assert!(!ChatSecurityLevel::Extreme.admits(SecurityLevel::Ultra));
    }

    /// The names are the UI's stored strings; a rename here would break its push.
    #[test]
    fn levels_cross_the_wire_by_the_uis_names() {
        let wire = serde_json::to_string(&ChatSecurityLevel::Reinforced).unwrap();
        assert_eq!(wire, r#""Reinforced""#);
    }
}
