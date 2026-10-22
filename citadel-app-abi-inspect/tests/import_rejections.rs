//! Rejections of a module's imports and of its bytes, each with its control: the same module
//! with the one defect removed.

mod support;

use citadel_app_abi_inspect::{check, Rejection};
use citadel_app_sdk_types::{Capability, Group};
use support::*;

#[test]
fn the_valid_module_is_accepted_and_reports_its_boundary() {
    let v = judge(&Parts::valid());
    let citadel_app_abi_inspect::Verdict::Accepted(insp) = v else {
        panic!("{v:?}")
    };
    assert_eq!(insp.imports.len(), 4);
    assert_eq!(insp.memory().unwrap().initial_pages, 1);
    assert_eq!(insp.exports.len(), 6);
}

#[test]
fn bytes_that_are_not_a_core_module_are_malformed() {
    let v = check(b"not wasm at all", &policy(&[]));
    assert!(matches!(v.reasons(), [Rejection::Malformed(_)]), "{v:?}");
    let component = wat::parse_str("(component)").unwrap();
    assert!(matches!(
        check(&component, &policy(&[])).reasons(),
        [Rejection::Malformed(_)]
    ));
    assert_accepted(&Parts::valid());
}

#[test]
fn atomics_are_threads() {
    let mut p = Parts::valid();
    p.extra = "(func (param i32) (result i32) local.get 0 i32.atomic.load)".into();
    assert_rejected_with(&p, &Rejection::Threads);
    p.extra = "(func (param i32) (result i32) local.get 0 i32.load)".into();
    assert_accepted(&p);
}

#[test]
fn imports_outside_the_namespace_are_foreign() {
    for (module, name) in [
        ("env", "fetch"),
        ("__wbindgen_placeholder__", "__wbindgen_describe"),
        ("wasi_snapshot_preview1", "fd_write"),
    ] {
        let mut p = Parts::valid();
        p.imports +=
            &format!(r#"(import "{module}" "{name}" (func (param i32 i32 i32) (result i32)))"#);
        assert_rejected_with(
            &p,
            &Rejection::ForeignImport {
                module: module.into(),
                name: name.into(),
            },
        );
    }
    assert_accepted(&Parts::valid());
}

#[test]
fn a_name_the_abi_does_not_have_is_unknown() {
    let mut p = Parts::valid();
    p.imports += r#"(import "citadel_app_abi_1" "fetch" (func (param i32 i32 i32) (result i32)))"#;
    assert_rejected_with(
        &p,
        &Rejection::UnknownImport {
            name: "fetch".into(),
        },
    );
    let mut control = Parts::valid();
    control.imports +=
        r#"(import "citadel_app_abi_1" "log" (func (param i32 i32 i32) (result i32)))"#;
    let v = check(
        &control.wasm(),
        &policy(&[Capability::Canvas, Capability::Log]),
    );
    assert!(v.is_accepted(), "{v:?}");
}

#[test]
fn an_abi_name_imported_as_the_wrong_kind_or_signature_is_refused() {
    let mut p = Parts::valid();
    p.imports = format!(r#"(import "citadel_app_abi_1" "canvas" (global i32)){REPLIES}"#);
    assert_rejected_with(
        &p,
        &Rejection::WrongImportKind {
            name: "canvas".into(),
        },
    );
    p.imports = format!(
        r#"(import "citadel_app_abi_1" "canvas" (func (param i32) (result i32))){REPLIES}"#
    );
    assert_rejected_with(
        &p,
        &Rejection::WrongImportSignature {
            name: "canvas".into(),
        },
    );
    p.imports = format!(
        r#"{CANVAS}(import "citadel_app_abi_1" "reply_len" (func (param i32 i32 i32) (result i32)))"#
    );
    assert_rejected_with(
        &p,
        &Rejection::WrongImportSignature {
            name: "reply_len".into(),
        },
    );
    p.imports = format!(r#"{CANVAS}{REPLIES}(import "citadel_app_abi_1" "memory" (func))"#);
    assert_rejected_with(
        &p,
        &Rejection::WrongImportKind {
            name: "memory".into(),
        },
    );
    assert_accepted(&Parts::valid());
}

#[test]
fn a_group_the_manifest_does_not_declare_is_refused_and_declaring_it_admits_the_same_module() {
    let mut p = Parts::valid();
    p.imports += r#"(import "citadel_app_abi_1" "net" (func (param i32 i32 i32) (result i32)))"#;
    let wasm = p.wasm();
    let refused = check(&wasm, &policy(&[Capability::Canvas]));
    assert_eq!(
        refused.reasons(),
        [Rejection::UndeclaredGroup { group: Group::Net }]
    );
    for opener in [
        Capability::NetHost,
        Capability::NetHostPrivate,
        Capability::NetServer,
    ] {
        let v = check(&wasm, &policy(&[Capability::Canvas, opener]));
        assert!(v.is_accepted(), "{opener:?} opens net: {v:?}");
    }
    let wrong_group = check(
        &wasm,
        &policy(&[Capability::Canvas, Capability::StorageUser]),
    );
    assert_eq!(
        wrong_group.reasons(),
        [Rejection::UndeclaredGroup { group: Group::Net }]
    );
}

#[test]
fn intrinsic_groups_need_no_declaration_and_others_do() {
    for intrinsic in ["ui", "events", "navigate", "log", "canvas"] {
        let mut p = Parts::valid();
        p.imports = format!(
            r#"(import "citadel_app_abi_1" "{intrinsic}" (func (param i32 i32 i32) (result i32))){REPLIES}"#
        );
        let v = check(&p.wasm(), &policy(&[]));
        assert!(v.is_accepted(), "{intrinsic}: {v:?}");
    }
    let mut p = Parts::valid();
    p.imports = format!(
        r#"(import "citadel_app_abi_1" "nodes" (func (param i32 i32 i32) (result i32))){REPLIES}"#
    );
    assert_eq!(
        check(&p.wasm(), &policy(&[])).reasons(),
        [Rejection::UndeclaredGroup {
            group: Group::Nodes
        }]
    );
}
