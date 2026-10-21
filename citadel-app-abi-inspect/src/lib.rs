//! Is this wasm a Citadel app the host may instantiate? Pure: bytes and the manifest's declared
//! capabilities in, an [`Inspection`] and a list of [`Rejection`]s out. No I/O, no instantiation.
//!
//! The host enforces the same rules at load (`WebAssembly.Module.imports`); this is the Rust
//! statement of them, for the kernel at publish/install time and for CI over the catalogue.

mod model;
mod parse;
mod rules;

pub use model::{
    ExportKind, FuncSig, Import, ImportKind, Inspection, MemoryDecl, Rejection, ValKind,
};
pub use parse::inspect;
pub use rules::verdict;

use citadel_app_sdk_types::Capability;

/// What the module is judged against. Every field is explicit: there is no default limit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    /// The capabilities the manifest declares (not what an install granted). The intrinsic ones
    /// (`Capability::INTRINSIC`) count as declared whether listed or not, as the host's do.
    pub declared: Vec<Capability>,
    /// The memory ceiling, in 64 KiB pages, the host will construct the memory with.
    pub max_memory_pages: u32,
}

/// The whole judgement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Accepted(Inspection),
    /// `inspection` is `None` when the bytes are not a readable core module at all.
    Rejected {
        inspection: Option<Inspection>,
        reasons: Vec<Rejection>,
    },
}

impl Verdict {
    pub fn is_accepted(&self) -> bool {
        matches!(self, Verdict::Accepted(_))
    }

    pub fn reasons(&self) -> &[Rejection] {
        match self {
            Verdict::Accepted(_) => &[],
            Verdict::Rejected { reasons, .. } => reasons,
        }
    }
}

/// Inspect `wasm` and judge it against `policy`.
pub fn check(wasm: &[u8], policy: &Policy) -> Verdict {
    match inspect(wasm) {
        Err(reason) => Verdict::Rejected {
            inspection: None,
            reasons: vec![reason],
        },
        Ok(inspection) => {
            let reasons = verdict(&inspection, policy);
            if reasons.is_empty() {
                Verdict::Accepted(inspection)
            } else {
                Verdict::Rejected {
                    inspection: Some(inspection),
                    reasons,
                }
            }
        }
    }
}
