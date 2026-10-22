//! A valid ABI module in WAT, built from parts, so each test changes exactly one part and its
//! control is the same module with that part restored. Each test binary that includes this
//! module uses a different subset of it, hence the dead-code allowance.
#![allow(dead_code)]

use citadel_app_abi_inspect::{check, Policy, Rejection, Verdict};
use citadel_app_sdk_types::Capability;

pub const MEMORY: &str = r#"(import "citadel_app_abi_1" "memory" (memory 1))"#;
pub const CANVAS: &str =
    r#"(import "citadel_app_abi_1" "canvas" (func (param i32 i32 i32) (result i32)))"#;
pub const REPLIES: &str = r#"
  (import "citadel_app_abi_1" "reply_len" (func (param i32) (result i32)))
  (import "citadel_app_abi_1" "reply_read" (func (param i32 i32 i32) (result i32)))"#;
pub const ALLOC: &str = r#"(func (export "app_alloc") (param i32) (result i32) i32.const 0)"#;
pub const FREE: &str = r#"(func (export "app_free") (param i32 i32))"#;
pub const INIT: &str = r#"(func (export "app_init") (param i32 i32) (result i32) i32.const 0)"#;
pub const EVENT: &str = r#"(func (export "app_event") (param i32 i32))"#;
pub const FRAME: &str = r#"(func (export "app_frame") (param i32))"#;
pub const CLOSE: &str = r#"(func (export "app_close") (param i32) (result i32) i32.const 0)"#;

#[derive(Clone)]
pub struct Parts {
    pub memory: String,
    pub imports: String,
    pub exports: Vec<String>,
    pub extra: String,
}

impl Parts {
    pub fn valid() -> Parts {
        Parts {
            memory: MEMORY.into(),
            imports: format!("{CANVAS}{REPLIES}"),
            exports: [ALLOC, FREE, INIT, EVENT, CLOSE, FRAME]
                .map(String::from)
                .to_vec(),
            extra: String::new(),
        }
    }

    pub fn wat(&self) -> String {
        format!(
            "(module\n  {}\n  {}\n  {}\n  {}\n)",
            self.memory,
            self.imports,
            self.exports.join("\n  "),
            self.extra
        )
    }

    pub fn wasm(&self) -> Vec<u8> {
        wat::parse_str(self.wat()).unwrap_or_else(|e| panic!("bad test WAT: {e}\n{}", self.wat()))
    }

    pub fn without_export(mut self, export: &str) -> Parts {
        self.exports.retain(|e| e != export);
        self
    }
}

pub fn policy(declared: &[Capability]) -> Policy {
    Policy {
        declared: declared.to_vec(),
        max_memory_pages: 16,
    }
}

/// The verdict on `parts` under a policy declaring only `canvas`.
pub fn judge(parts: &Parts) -> Verdict {
    check(&parts.wasm(), &policy(&[Capability::Canvas]))
}

pub fn assert_rejected_with(parts: &Parts, reason: &Rejection) {
    let v = judge(parts);
    assert!(
        v.reasons().contains(reason),
        "expected {reason:?}, got {v:?}\n{}",
        parts.wat()
    );
}

pub fn assert_accepted(parts: &Parts) {
    let v = judge(parts);
    assert!(
        v.is_accepted(),
        "the control must pass: {v:?}\n{}",
        parts.wat()
    );
}
