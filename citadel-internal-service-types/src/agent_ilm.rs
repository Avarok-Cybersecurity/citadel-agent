//! Agent-hosted ILM: what an agent offers, and the answers to the two requests
//! by which a session uses it (docs/plans/multi-browser-cid.md, phase 2b).
//!
//! An agent run with `multi_subscriber` on can host an account's ILM itself
//! instead of the browser. It says so in `GetSessionsResponse::agent_ilm`, the
//! response the browser's messenger already polls; an agent without the
//! setting, and every older agent, leaves that field `None`.
//!
//! A session opts in with `InternalServiceRequest::EnableAgentIlm`, and then
//! sends through the agent's ILM with `InternalServiceRequest::SendReliable`.
//! Each is answered by its own Success/Failure pair below, correlated by
//! `request_id`.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg(feature = "typescript")]
use ts_rs::TS;

/// Present only when this agent offers agent-hosted ILM.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct AgentIlmOffer {
    /// The accounts whose ILM this agent is hosting right now, ascending. A
    /// browser that finds its CID here must not start its own ILM for it: the
    /// agent's survives a page reload, and two ILMs must never run for one
    /// account.
    #[cfg_attr(feature = "typescript", ts(type = "Array<bigint>"))]
    pub hosted: Vec<u64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct EnableAgentIlmSuccess {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    /// True when the agent was already hosting this account's ILM, which is
    /// what a reloaded page finds. The opt-in is idempotent: either way the
    /// agent's ILM is the one running when this arrives.
    pub already_hosted: bool,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct EnableAgentIlmFailure {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub message: String,
    pub request_id: Option<Uuid>,
}

/// The message is in the agent's durable outbound queue; ILM delivers it and
/// retries until the peer acknowledges it. Not a delivery receipt.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct SendReliableSuccess {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub peer_cid: u64,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct SendReliableFailure {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub peer_cid: u64,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[cfg(test)]
#[path = "agent_ilm_tests.rs"]
mod tests;
