//! What a module declares at its boundary, and every way it can fail the ABI.

use citadel_app_sdk_types::Group;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValKind {
    I32,
    I64,
    F32,
    F64,
    V128,
    Ref,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuncSig {
    pub params: Vec<ValKind>,
    pub results: Vec<ValKind>,
}

impl FuncSig {
    pub fn of(params: &[ValKind], results: &[ValKind]) -> FuncSig {
        FuncSig {
            params: params.to_vec(),
            results: results.to_vec(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryDecl {
    pub initial_pages: u64,
    pub maximum_pages: Option<u64>,
    pub shared: bool,
    pub memory64: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportKind {
    Func(FuncSig),
    Memory(MemoryDecl),
    Table,
    Global,
    Tag,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Import {
    pub module: String,
    pub name: String,
    pub kind: ImportKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportKind {
    Func(FuncSig),
    Memory,
    Table,
    Global,
    Tag,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inspection {
    pub imports: Vec<Import>,
    pub exports: Vec<(String, ExportKind)>,
    /// Memories the module defines itself (the ABI allows none).
    pub defined_memories: Vec<MemoryDecl>,
    pub has_start: bool,
    /// The code uses atomics or shared memory: it validates only with threads enabled.
    pub uses_threads: bool,
}

impl Inspection {
    /// The imported ABI memory, if the module imports it.
    pub fn memory(&self) -> Option<MemoryDecl> {
        self.imports.iter().find_map(|i| match i.kind {
            ImportKind::Memory(m) if i.module == citadel_app_sdk_types::abi::NAMESPACE => Some(m),
            _ => None,
        })
    }
}

/// Why a module is refused. Each is its own class with its own test and control.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rejection {
    /// Not a valid wasm core module under the ABI's feature set (wasm 2.0).
    Malformed(String),
    /// Atomics or shared memory: no threads in an app.
    Threads,
    /// An import from any module but `citadel_app_abi_1` (`env`, `wasi_*`, `__wbindgen_*`).
    ForeignImport {
        module: String,
        name: String,
    },
    /// An import in the namespace that is not one of the ABI's names.
    UnknownImport {
        name: String,
    },
    /// An ABI name imported as the wrong kind (a table, a global, a function named `memory`).
    WrongImportKind {
        name: String,
    },
    WrongImportSignature {
        name: String,
    },
    /// A group import the manifest declares no capability for.
    UndeclaredGroup {
        group: Group,
    },
    /// The memory is not imported from the namespace, so the host cannot cap it.
    MemoryNotImported,
    /// The module defines a memory of its own.
    MemoryDefined,
    SharedMemory,
    Memory64,
    /// The imported memory's minimum already exceeds the host's ceiling.
    MemoryInitialTooLarge {
        initial_pages: u64,
        max_pages: u32,
    },
    MissingExport {
        name: String,
    },
    WrongExportSignature {
        name: String,
    },
    /// A start function runs before the host has initialised anything.
    StartFunction,
}

impl Rejection {
    /// A stable, machine-readable form: the class in snake_case, then `:` and what it names
    /// (`undeclared_group:net`, `foreign_import:env.fetch`). Fixtures and hosts compare these.
    pub fn code(&self) -> String {
        match self {
            Rejection::Malformed(_) => "malformed".into(),
            Rejection::Threads => "threads".into(),
            Rejection::ForeignImport { module, name } => format!("foreign_import:{module}.{name}"),
            Rejection::UnknownImport { name } => format!("unknown_import:{name}"),
            Rejection::WrongImportKind { name } => format!("wrong_import_kind:{name}"),
            Rejection::WrongImportSignature { name } => format!("wrong_import_signature:{name}"),
            Rejection::UndeclaredGroup { group } => {
                format!("undeclared_group:{}", group.import_name())
            }
            Rejection::MemoryNotImported => "memory_not_imported".into(),
            Rejection::MemoryDefined => "memory_defined".into(),
            Rejection::SharedMemory => "shared_memory".into(),
            Rejection::Memory64 => "memory64".into(),
            Rejection::MemoryInitialTooLarge { .. } => "memory_initial_too_large".into(),
            Rejection::MissingExport { name } => format!("missing_export:{name}"),
            Rejection::WrongExportSignature { name } => format!("wrong_export_signature:{name}"),
            Rejection::StartFunction => "start_function".into(),
        }
    }
}
