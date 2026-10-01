//! An account's 1:1 conversations, kept by the agent (multi-window mw4).
//!
//! The agent is the only writer: every window reads and renders, and asks the
//! agent for every change, which it applies under one lock per conversation and
//! announces as a `ConversationEvent` to every window attached to the session.
//! Two windows can no longer read-modify-write the same page and lose a write,
//! and a message that arrives with no window open is stored all the same.

pub(crate) mod cbor;
pub(crate) mod command;
pub(crate) mod engine;
mod envelope;
mod inbound;
mod inbound_notice;
pub(crate) mod io;
pub(crate) mod kv;
mod outbound;
mod outbound_more;
pub(crate) mod reactions;
mod requests;
mod requests_read;
pub(crate) mod retention;
pub(crate) mod store;
mod store_lifecycle;
pub(crate) mod store_mutations;
mod store_rules;
pub(crate) mod stored;

pub(crate) use engine::Engine;
pub(crate) use io::ConversationIo;

#[cfg(test)]
#[path = "engine_fake.rs"]
mod engine_fake;
#[cfg(test)]
#[path = "engine_tests.rs"]
mod engine_tests;
#[cfg(test)]
#[path = "engine_tests_notices.rs"]
mod engine_tests_notices;
