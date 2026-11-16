//! Fixtures shared by the integration tests. Every seed and nonce is a fixed test value, so a
//! failure reproduces; none is, or derives, a real key.
#![allow(dead_code)]

use citadel_identity::{canonical_domain, CanonicalDomain};

pub const SEED: [u8; 32] = [7; 32];
pub const OTHER_SEED: [u8; 32] = [9; 32];
pub const SERVER_SEED: [u8; 32] = [11; 32];
pub const ATTACKER_SEED: [u8; 32] = [13; 32];
pub const NONCE: [u8; 32] = [0x5a; 32];
/// 2026-11-16T00:00:00Z, the tests' "now".
pub const NOW: u64 = 1_794_700_800;

pub fn domain(text: &str) -> CanonicalDomain {
    canonical_domain(text).unwrap()
}

pub fn main_domain() -> CanonicalDomain {
    domain("main.work.avarok.net")
}

pub fn former_domain() -> CanonicalDomain {
    domain("main-old.work.avarok.net")
}

pub mod kat;
