//! Reading the module: validation under the ABI's feature set, then its boundary.

use crate::model::{
    ExportKind, FuncSig, Import, ImportKind, Inspection, MemoryDecl, Rejection, ValKind,
};
use wasmparser::{
    CompositeInnerType, Encoding, ExternalKind, FuncType, MemoryType, Parser, Payload, TypeRef,
    ValType, Validator, WasmFeatures,
};

/// Wasm 2.0: what browsers run and what rustc emits for wasm32-unknown-unknown. No threads,
/// memory64, multi-memory, exceptions, GC or compact imports.
const ABI_FEATURES: WasmFeatures = WasmFeatures::WASM2;

fn malformed(e: impl std::fmt::Display) -> Rejection {
    Rejection::Malformed(e.to_string())
}

/// Ok(true) when the module is valid only once threads (atomics, shared memory) are allowed.
fn uses_threads(wasm: &[u8]) -> Result<bool, Rejection> {
    let Err(e) = Validator::new_with_features(ABI_FEATURES).validate_all(wasm) else {
        return Ok(false);
    };
    let with_threads = ABI_FEATURES.union(WasmFeatures::THREADS);
    match Validator::new_with_features(with_threads).validate_all(wasm) {
        Ok(_) => Ok(true),
        Err(_) => Err(malformed(e)),
    }
}

fn val(v: &ValType) -> ValKind {
    match v {
        ValType::I32 => ValKind::I32,
        ValType::I64 => ValKind::I64,
        ValType::F32 => ValKind::F32,
        ValType::F64 => ValKind::F64,
        ValType::V128 => ValKind::V128,
        ValType::Ref(_) => ValKind::Ref,
    }
}

fn sig(f: &FuncType) -> FuncSig {
    FuncSig {
        params: f.params().iter().map(val).collect(),
        results: f.results().iter().map(val).collect(),
    }
}

fn memory(m: MemoryType) -> MemoryDecl {
    MemoryDecl {
        initial_pages: m.initial,
        maximum_pages: m.maximum,
        shared: m.shared,
        memory64: m.memory64,
    }
}

fn func_sig(types: &[Option<FuncSig>], type_index: u32) -> Result<FuncSig, Rejection> {
    types
        .get(type_index as usize)
        .cloned()
        .flatten()
        .ok_or_else(|| malformed(format!("type {type_index} is not a function type")))
}

/// The module's boundary. `Err` only when it is not a readable core module.
pub fn inspect(wasm: &[u8]) -> Result<Inspection, Rejection> {
    let uses_threads = uses_threads(wasm)?;
    let mut types: Vec<Option<FuncSig>> = Vec::new();
    let mut func_types: Vec<u32> = Vec::new();
    let mut imports = Vec::new();
    let mut raw_exports = Vec::new();
    let mut defined_memories = Vec::new();
    let mut has_start = false;
    for payload in Parser::new(0).parse_all(wasm) {
        match payload.map_err(malformed)? {
            Payload::Version {
                encoding: Encoding::Component,
                ..
            } => return Err(malformed("a component, not a core module")),
            Payload::TypeSection(r) => {
                for group in r {
                    for sub in group.map_err(malformed)?.into_types() {
                        types.push(match &sub.composite_type.inner {
                            CompositeInnerType::Func(f) => Some(sig(f)),
                            _ => None,
                        });
                    }
                }
            }
            Payload::ImportSection(r) => {
                for import in r.into_imports() {
                    let import = import.map_err(malformed)?;
                    let kind = match import.ty {
                        TypeRef::Func(t) | TypeRef::FuncExact(t) => {
                            func_types.push(t);
                            ImportKind::Func(func_sig(&types, t)?)
                        }
                        TypeRef::Memory(m) => ImportKind::Memory(memory(m)),
                        TypeRef::Table(_) => ImportKind::Table,
                        TypeRef::Global(_) => ImportKind::Global,
                        TypeRef::Tag(_) => ImportKind::Tag,
                    };
                    imports.push(Import {
                        module: import.module.into(),
                        name: import.name.into(),
                        kind,
                    });
                }
            }
            Payload::FunctionSection(r) => {
                for t in r {
                    func_types.push(t.map_err(malformed)?);
                }
            }
            Payload::MemorySection(r) => {
                for m in r {
                    defined_memories.push(memory(m.map_err(malformed)?));
                }
            }
            Payload::ExportSection(r) => {
                for e in r {
                    let e = e.map_err(malformed)?;
                    raw_exports.push((e.name.to_string(), e.kind, e.index));
                }
            }
            Payload::StartSection { .. } => has_start = true,
            _ => {}
        }
    }
    let mut exports = Vec::new();
    for (name, kind, index) in raw_exports {
        let kind = match kind {
            ExternalKind::Func | ExternalKind::FuncExact => {
                let t = func_types
                    .get(index as usize)
                    .ok_or_else(|| malformed(format!("export {name} names no function")))?;
                ExportKind::Func(func_sig(&types, *t)?)
            }
            ExternalKind::Memory => ExportKind::Memory,
            ExternalKind::Table => ExportKind::Table,
            ExternalKind::Global => ExportKind::Global,
            ExternalKind::Tag => ExportKind::Tag,
        };
        exports.push((name, kind));
    }
    Ok(Inspection {
        imports,
        exports,
        defined_memories,
        has_start,
        uses_threads,
    })
}
