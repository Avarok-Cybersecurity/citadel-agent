//! Loading the frozen batch-1116 contracts (`docs/contracts/batch-1116/` in the parent repo).
//! `tests/contracts/batch-1116/` holds byte-identical copies, because this repository's CI
//! builds without the parent checked out; the parent's gate is what keeps the two equal.

use serde_json::Value;
use std::path::PathBuf;

pub fn contract_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/contracts/batch-1116")
        .join(relative)
}

pub fn read_bytes(relative: &str) -> Vec<u8> {
    let path = contract_path(relative);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

pub fn load(relative: &str) -> Value {
    serde_json::from_slice(&read_bytes(relative)).unwrap()
}

pub fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("{key} is not text in {value}"))
}

pub fn number(value: &Value, key: &str) -> u64 {
    value[key]
        .as_u64()
        .unwrap_or_else(|| panic!("{key} is not an unsigned integer in {value}"))
}

pub fn hex(value: &Value, key: &str) -> Vec<u8> {
    let text = text(value, key);
    assert!(text.len().is_multiple_of(2), "{key}: odd hex length");
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
        .collect()
}

pub fn hex32(value: &Value, key: &str) -> [u8; 32] {
    hex(value, key).try_into().unwrap()
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The variant name of an error's `Debug` form, which is what the KAT's `error` fields name.
pub fn variant<E: std::fmt::Debug>(error: &E) -> String {
    let debug = format!("{error:?}");
    debug.split(['(', ' ', '{']).next().unwrap().to_string()
}

pub fn strings(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect()
}
