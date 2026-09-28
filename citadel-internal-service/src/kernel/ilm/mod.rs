//! An ILM hosted by the agent, one per account (phase 2a of
//! docs/plans/multi-browser-cid.md). Built from the SAME backend, frames and
//! delivery the browser's WASM messenger uses, so the two hosts share state
//! and wire format; not yet wired into any request path.
// ILM's `Backend`/transport error type is fixed as `BackendError<T>` /
// `NetworkError<T>`, which carry the undelivered message inline (~288 bytes
// with T = WrappedMessage). The return types are dictated by ILM, so the lint
// cannot be satisfied here without changing ILM's public error enums -- the
// same reasoning, and the same module-scoped allow, as the connector's
// messenger/mod.rs.
#![allow(clippy::result_large_err)]

pub mod host;
pub mod kv;
pub mod transport;

#[cfg(test)]
mod tests;
