//! `citadel-vfs`: one virtual filesystem's model, canonical op codec, reducer, permission
//! function, derived views and migration passes (`docs/contracts/rel-1124/`). The agent, the
//! kernel and the page link the same code.
//!
//! Pure: no I/O, no clock, no randomness. Times, ids and random bytes are arguments.
//! Hashes: SHA3-256 for ids, op hashes and chains; signatures: ML-DSA-65.

#![forbid(unsafe_code)]

pub mod authorize;
pub mod cbor;
mod hash;
pub mod inode;
pub mod limits;
pub mod model;
pub mod name;
pub mod op;
pub mod principal;
pub mod reduce;
pub mod volume;

pub use hash::{from_hex, sha3_256, to_hex};
pub use inode::{InodeId, Namespace};
pub use principal::Principal;
pub use volume::{VolumeId, VolumeKind, VolumeMode};
