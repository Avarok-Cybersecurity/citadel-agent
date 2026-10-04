//! Keeping every hosted account's links alive, with or without a window open.
//!
//! The decisions are `core` (pure, tested with a manual clock); `ports` are what it needs
//! from the world; `adapters` are the production ends of those ports; `shell` is the loop
//! that joins them and holds no decisions; `registry` starts one per hosted account beside
//! its ILM and stops it on the same truly-ended paths.
//! Design: citadel-workspace `docs/plans/connection-supervisor.md`.

mod adapters;
mod core;
mod policy;
mod ports;
mod registry;
mod shell;
mod shell_effects;
#[cfg(test)]
mod shell_rig;
#[cfg(test)]
mod shell_tests;
mod types;

pub use policy::{Backoff, SupervisorPolicy, AGENT_SUPERVISOR};
pub(crate) use registry::Supervisors;
pub(crate) use shell::Signal;
pub(crate) use types::LinkStatus;
