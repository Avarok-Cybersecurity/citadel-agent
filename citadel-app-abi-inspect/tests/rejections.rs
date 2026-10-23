//! Rejections of a module's memory, exports and start section, each with its control: the
//! same module with the one defect removed.

mod support;

use citadel_app_abi_inspect::Rejection;
use citadel_app_sdk_types::{Capability, Group};
use support::*;

#[test]
fn a_memory_that_is_not_imported_cannot_be_capped() {
    let mut defined = Parts::valid();
    defined.memory = String::new();
    defined.extra = "(memory 1)".into();
    assert_eq!(
        judge(&defined).reasons(),
        [Rejection::MemoryNotImported, Rejection::MemoryDefined]
    );
    let mut none = Parts::valid();
    none.memory = String::new();
    assert_eq!(judge(&none).reasons(), [Rejection::MemoryNotImported]);
    let mut env = Parts::valid();
    env.memory = r#"(import "env" "memory" (memory 1))"#.into();
    let reasons = judge(&env).reasons().to_vec();
    assert!(
        reasons.contains(&Rejection::MemoryNotImported),
        "{reasons:?}"
    );
    assert_accepted(&Parts::valid());
}

#[test]
fn shared_memory_is_refused() {
    let mut p = Parts::valid();
    p.memory = r#"(import "citadel_app_abi_1" "memory" (memory 1 2 shared))"#.into();
    assert_rejected_with(&p, &Rejection::SharedMemory);
    p.memory = r#"(import "citadel_app_abi_1" "memory" (memory 1 2))"#.into();
    assert_accepted(&p);
}

#[test]
fn a_minimum_above_the_ceiling_is_refused_and_the_ceiling_itself_is_not() {
    let mut p = Parts::valid();
    p.memory = r#"(import "citadel_app_abi_1" "memory" (memory 17))"#.into();
    assert_eq!(
        judge(&p).reasons(),
        [Rejection::MemoryInitialTooLarge {
            initial_pages: 17,
            max_pages: 16
        }]
    );
    p.memory = r#"(import "citadel_app_abi_1" "memory" (memory 16))"#.into();
    assert_accepted(&p);
}

#[test]
fn each_required_export_is_required_and_frame_is_optional() {
    for export in [ALLOC, FREE, INIT, EVENT, CLOSE] {
        let p = Parts::valid().without_export(export);
        let name = export.split('"').nth(1).unwrap();
        assert_eq!(
            judge(&p).reasons(),
            [Rejection::MissingExport { name: name.into() }]
        );
    }
    assert_accepted(&Parts::valid().without_export(FRAME));
}

#[test]
fn an_export_with_the_wrong_signature_is_refused() {
    let mut p = Parts::valid().without_export(INIT);
    p.exports
        .push(r#"(func (export "app_init") (param i32 i32))"#.into());
    assert_eq!(
        judge(&p).reasons(),
        [Rejection::WrongExportSignature {
            name: "app_init".into()
        }]
    );
    let mut f = Parts::valid().without_export(FRAME);
    f.exports
        .push(r#"(func (export "app_frame") (param f64))"#.into());
    assert_eq!(
        judge(&f).reasons(),
        [Rejection::WrongExportSignature {
            name: "app_frame".into()
        }]
    );
    // app_close answers a CloseResult: one that answers nothing cannot say it ran.
    let mut c = Parts::valid().without_export(CLOSE);
    c.exports
        .push(r#"(func (export "app_close") (param i32))"#.into());
    assert_eq!(
        judge(&c).reasons(),
        [Rejection::WrongExportSignature {
            name: "app_close".into()
        }]
    );
    let mut g = Parts::valid().without_export(EVENT);
    g.exports
        .push(r#"(global (export "app_event") i32 (i32.const 0))"#.into());
    assert_eq!(
        judge(&g).reasons(),
        [Rejection::WrongExportSignature {
            name: "app_event".into()
        }]
    );
    assert_accepted(&Parts::valid());
}

#[test]
fn a_start_function_is_refused() {
    let mut p = Parts::valid();
    p.extra = "(func $s) (start $s)".into();
    assert_eq!(judge(&p).reasons(), [Rejection::StartFunction]);
    p.extra = "(func $s)".into();
    assert_accepted(&p);
}

#[test]
fn memory64_is_refused_by_the_rules_even_if_a_parser_admitted_it() {
    let citadel_app_abi_inspect::Verdict::Accepted(mut insp) = judge(&Parts::valid()) else {
        panic!()
    };
    let pol = policy(&[Capability::Canvas]);
    assert!(citadel_app_abi_inspect::verdict(&insp, &pol).is_empty());
    for i in insp.imports.iter_mut() {
        if let citadel_app_abi_inspect::ImportKind::Memory(m) = &mut i.kind {
            m.memory64 = true;
        }
    }
    assert_eq!(
        citadel_app_abi_inspect::verdict(&insp, &pol),
        [Rejection::Memory64]
    );
    // And the real encoding is not even a wasm 2.0 module.
    let mut p = Parts::valid();
    p.memory = r#"(import "citadel_app_abi_1" "memory" (memory i64 1))"#.into();
    assert!(matches!(judge(&p).reasons(), [Rejection::Malformed(_)]));
}

#[test]
fn rejection_codes_are_stable_strings() {
    assert_eq!(
        Rejection::UndeclaredGroup { group: Group::Net }.code(),
        "undeclared_group:net"
    );
    let foreign = Rejection::ForeignImport {
        module: "env".into(),
        name: "fetch".into(),
    };
    assert_eq!(foreign.code(), "foreign_import:env.fetch");
    assert_eq!(Rejection::StartFunction.code(), "start_function");
    assert_eq!(Rejection::Malformed("x".into()).code(), "malformed");
}
