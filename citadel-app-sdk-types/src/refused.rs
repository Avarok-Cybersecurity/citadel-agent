//! A host reply: the op's value, or why the host would not perform it.

use alloc::string::String;
use serde::{Deserialize, Serialize};

/// Why a call was not performed. CBOR: `"ungranted"`, `{"unavailable": "<reason>"}`,
/// `{"limit": "<what>"}`, `{"policy": "<reason>"}` or `{"remote": <status>}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum Refused {
    /// The install was not granted the capability this op needs.
    Ungranted,
    /// The capability is granted but cannot be served here (no agent, no host advertising, ...).
    Unavailable(String),
    /// A quota, size or rate limit; the string names which.
    Limit(String),
    /// Refused by policy (egress allow-list, scope, the acting user's own permission).
    Policy(String),
    /// The remote end answered with this status.
    Remote(u16),
}

/// What a reply handle holds. CBOR: `{"ok": <reply>}` or `{"refused": <Refused>}`, so the
/// ungranted stub is exactly `{refused: "ungranted"}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum HostReply<T> {
    Ok(T),
    Refused(Refused),
}

impl<T> HostReply<T> {
    pub fn into_result(self) -> Result<T, Refused> {
        match self {
            HostReply::Ok(v) => Ok(v),
            HostReply::Refused(r) => Err(r),
        }
    }
}
