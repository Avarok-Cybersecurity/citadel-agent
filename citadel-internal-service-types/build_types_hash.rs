//! `CITADEL_TYPES_HASH`: the identity of this crate's wire types, compiled into
//! every binary that uses them (the agent and the browser's WASM client).
//!
//! It is SHA3-256 over the committed ts-rs output (`typescript-client/src/types`),
//! comment lines removed, files by sorted name. The CI gate
//! `ci/check-ts-bindings-fresh.sh` proves that output is exactly what the Rust
//! generates, so this hash changes exactly when a wire type's shape does, and
//! not when a doc comment does. The agent says it in its greeting and the page
//! compares it with its own, so two builds of different types say so up front
//! instead of failing deep in a decode.
//!
//! No fallback: a checkout without the bindings fails the build here, rather
//! than ship a binary that claims a hash it never computed. The one exception is
//! the `typescript` feature, which is the build that WRITES the bindings
//! (generate_types.sh deletes and regenerates them) and ships nowhere: it is
//! given a value that names it, so it cannot pass for a real hash.
use sha3::{Digest, Sha3_256};
use std::fs;
use std::path::Path;

const BINDINGS: &str = "../typescript-client/src/types";

/// What a `typescript`-feature build says instead of a hash. Not hex, so the
/// page's comparison can never mistake it for one.
const GENERATOR_BUILD: &str = "ts-rs-generator-build";

pub fn emit() {
    if std::env::var_os("CARGO_FEATURE_TYPESCRIPT").is_some() {
        println!("cargo:rustc-env=CITADEL_TYPES_HASH={GENERATOR_BUILD}");
        return;
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(BINDINGS);
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut names: Vec<String> = fs::read_dir(&dir)
        .unwrap_or_else(|err| {
            panic!(
                "{} is unreadable ({err}); it is the source of CITADEL_TYPES_HASH",
                dir.display()
            )
        })
        .map(|entry| {
            entry
                .expect("a readable directory entry")
                .file_name()
                .into_string()
                .expect("a UTF-8 file name")
        })
        .filter(|name| name.ends_with(".ts"))
        .collect();
    assert!(
        !names.is_empty(),
        "{} holds no bindings to hash",
        dir.display()
    );
    names.sort();
    let mut hash = Sha3_256::new();
    for name in &names {
        let path = dir.join(name);
        println!("cargo:rerun-if-changed={}", path.display());
        let text =
            fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
        hash.update(name.as_bytes());
        hash.update([0]);
        for line in text.lines().filter(|line| !is_comment(line)) {
            hash.update(line.trim_end().as_bytes());
            hash.update(b"\n");
        }
        hash.update([0]);
    }
    let hex: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    println!("cargo:rustc-env=CITADEL_TYPES_HASH={hex}");
}

fn is_comment(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("//") || line.starts_with("/*") || line.starts_with('*')
}
