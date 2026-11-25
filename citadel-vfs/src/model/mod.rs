//! The entry model (`vfs-model.md` §4, §5): kinds, modes, pointees, admissions.

mod admission;
mod mode;
mod pointee;

pub use admission::{Admitted, AdmittedKey, Caps, ClaimWhat};
pub use mode::{Bit, Mode, Triple};
pub use pointee::Pointee;

use crate::principal::Principal;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EntryKind {
    Dir = 1,
    File = 2,
}

impl EntryKind {
    pub fn from_code(code: u64) -> Option<Self> {
        match code {
            1 => Some(Self::Dir),
            2 => Some(Self::File),
            _ => None,
        }
    }
}

/// `created` and `removal`: who and when, by the author's wall clock (never orders anything).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamped {
    pub by: Principal,
    pub at_ms: u64,
}
