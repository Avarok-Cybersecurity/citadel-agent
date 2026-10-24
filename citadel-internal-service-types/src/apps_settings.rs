//! A member's own grants, on this agent, to every app it runs for them.
//!
//! They live in the account's own LocalDB bucket under the agent's reserved `agent:` keys,
//! which no window may read or write raw: only `AppsGetAgentSettings` and
//! `AppsSetAgentSettings`, from a window attached to the session, reach them.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg(feature = "typescript")]
use ts_rs::TS;

/// This account's grants on this agent. `private_network` is false until the member turns it
/// on: an unset grant is not a grant.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct AppsAgentSettings {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub request_id: Option<Uuid>,
    /// "Let apps reach my private network from this computer".
    pub private_network: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct AppsAgentSettingsFailure {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub request_id: Option<Uuid>,
    pub message: String,
}
