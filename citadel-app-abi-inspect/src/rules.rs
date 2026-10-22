//! The ABI's rules over an [`Inspection`]: one function, every reason reported, in a fixed order.

use crate::model::{ExportKind, FuncSig, ImportKind, Inspection, Rejection, ValKind};
use crate::Policy;
use citadel_app_sdk_types::abi::{self, EXPORT_FRAME, NAMESPACE};
use citadel_app_sdk_types::{Capability, Group};

const I: ValKind = ValKind::I32;

/// `(op, ptr, len) -> handle`, the signature of every group import (and of `reply_read`).
fn group_sig() -> FuncSig {
    FuncSig::of(&[I, I, I], &[I])
}

fn reply_len_sig() -> FuncSig {
    FuncSig::of(&[I], &[I])
}

/// The required exports and their signatures, in `abi::REQUIRED_EXPORTS` order. `app_close`
/// is among them: a module without its graceful end is refused, not run without one.
fn required_exports() -> [(&'static str, FuncSig); 5] {
    [
        (abi::EXPORT_ALLOC, FuncSig::of(&[I], &[I])),
        (abi::EXPORT_FREE, FuncSig::of(&[I, I], &[])),
        (abi::EXPORT_INIT, FuncSig::of(&[I, I], &[I])),
        (abi::EXPORT_EVENT, FuncSig::of(&[I, I], &[])),
        (abi::EXPORT_CLOSE, FuncSig::of(&[I], &[I])),
    ]
}

fn frame_sig() -> FuncSig {
    FuncSig::of(&[I], &[])
}

fn push_once(out: &mut Vec<Rejection>, r: Rejection) {
    if !out.contains(&r) {
        out.push(r);
    }
}

fn expect_func(kind: &ImportKind, want: FuncSig, name: &str, out: &mut Vec<Rejection>) {
    match kind {
        ImportKind::Func(sig) if *sig == want => {}
        ImportKind::Func(_) => {
            push_once(out, Rejection::WrongImportSignature { name: name.into() })
        }
        _ => push_once(out, Rejection::WrongImportKind { name: name.into() }),
    }
}

fn imports(insp: &Inspection, policy: &Policy, out: &mut Vec<Rejection>) {
    for imp in &insp.imports {
        if imp.module != NAMESPACE {
            push_once(
                out,
                Rejection::ForeignImport {
                    module: imp.module.clone(),
                    name: imp.name.clone(),
                },
            );
            continue;
        }
        match imp.name.as_str() {
            abi::MEMORY => match imp.kind {
                ImportKind::Memory(m) => {
                    if m.shared {
                        push_once(out, Rejection::SharedMemory);
                    }
                    if m.memory64 {
                        push_once(out, Rejection::Memory64);
                    }
                    if m.initial_pages > u64::from(policy.max_memory_pages) {
                        let r = Rejection::MemoryInitialTooLarge {
                            initial_pages: m.initial_pages,
                            max_pages: policy.max_memory_pages,
                        };
                        push_once(out, r);
                    }
                }
                _ => push_once(
                    out,
                    Rejection::WrongImportKind {
                        name: imp.name.clone(),
                    },
                ),
            },
            abi::REPLY_LEN => expect_func(&imp.kind, reply_len_sig(), &imp.name, out),
            abi::REPLY_READ => expect_func(&imp.kind, group_sig(), &imp.name, out),
            name => match Group::from_import_name(name) {
                None => push_once(out, Rejection::UnknownImport { name: name.into() }),
                Some(group) => {
                    expect_func(&imp.kind, group_sig(), name, out);
                    let mut declared = policy.declared.iter().chain(Capability::INTRINSIC.iter());
                    if !declared.any(|c| c.group() == group) {
                        push_once(out, Rejection::UndeclaredGroup { group });
                    }
                }
            },
        }
    }
}

fn exports(insp: &Inspection, out: &mut Vec<Rejection>) {
    let find = |name: &str| insp.exports.iter().find(|(n, _)| n == name).map(|(_, k)| k);
    for (name, want) in required_exports() {
        match find(name) {
            None => out.push(Rejection::MissingExport { name: name.into() }),
            Some(ExportKind::Func(sig)) if *sig == want => {}
            Some(_) => out.push(Rejection::WrongExportSignature { name: name.into() }),
        }
    }
    match find(EXPORT_FRAME) {
        None => {}
        Some(ExportKind::Func(sig)) if *sig == frame_sig() => {}
        Some(_) => out.push(Rejection::WrongExportSignature {
            name: EXPORT_FRAME.into(),
        }),
    }
}

/// Every reason `insp` may not run under `policy`; empty means accepted.
pub fn verdict(insp: &Inspection, policy: &Policy) -> Vec<Rejection> {
    let mut out = Vec::new();
    if insp.uses_threads {
        out.push(Rejection::Threads);
    }
    imports(insp, policy, &mut out);
    if insp.memory().is_none() {
        out.push(Rejection::MemoryNotImported);
    }
    if !insp.defined_memories.is_empty() {
        out.push(Rejection::MemoryDefined);
    }
    if insp.has_start {
        out.push(Rejection::StartFunction);
    }
    exports(insp, &mut out);
    out
}
