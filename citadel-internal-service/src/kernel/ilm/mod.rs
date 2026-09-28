//! An ILM hosted by the agent, one per account (phase 2a of
//! docs/plans/multi-browser-cid.md). Built from the SAME backend, frames and
//! delivery the browser's WASM messenger uses, so the two hosts share state
//! and wire format. Offered only when the agent runs with `multi_subscriber`
//! on, and started per session by `EnableAgentIlm` (phase 2b).
// ILM's `Backend`/transport error type is fixed as `BackendError<T>` /
// `NetworkError<T>`, which carry the undelivered message inline (~288 bytes
// with T = WrappedMessage). The return types are dictated by ILM, so the lint
// cannot be satisfied here without changing ILM's public error enums -- the
// same reasoning, and the same module-scoped allow, as the connector's
// messenger/mod.rs.
#![allow(clippy::result_large_err)]

pub mod delivery;
pub mod host;
pub(crate) mod inbound;
pub mod kv;
pub mod service;
pub mod transport;

/// The kernel's agent-hosted ILM: the node's store, the session's peer
/// channels, delivery through the session's route.
pub(crate) type KernelAgentIlm<R> = service::AgentIlmService<
    kv::SharedDb,
    transport::SessionPeerLinks<R>,
    crate::kernel::session_route::SessionRoute,
>;

#[cfg(test)]
mod tests;
