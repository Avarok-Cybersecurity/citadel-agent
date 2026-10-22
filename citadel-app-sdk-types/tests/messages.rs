//! Context and events round-trip, and keep the tags the host matches on.

use citadel_app_sdk_types::calls::StorageBackend;
use citadel_app_sdk_types::{
    decode, encode, AppContext, AppEvent, AppScope, Capability, Size, Surface, SuspendReason,
};

fn context() -> AppContext {
    AppContext {
        app_id: "com.acme.kanban".into(),
        install_id: "i1".into(),
        scope: AppScope::NodeTree {
            node_id: "n1".into(),
        },
        granted: vec![Capability::Canvas, Capability::Log],
        surface: Surface::MdZone {
            zone_id: "z".into(),
        },
        size: Size {
            width: 640,
            height: 480,
        },
        dpr: 2.0,
        theme: [("bg".to_string(), "#fff".to_string())].into(),
        locale: "en".into(),
        acting_username: "alice".into(),
        node_id: "n2".into(),
        close_grace_ms: 500,
    }
}

#[test]
fn the_context_round_trips() {
    let ctx = context();
    assert_eq!(decode::<AppContext>(&encode(&ctx)).unwrap(), ctx);
    assert!(ctx.is_granted(Capability::Log));
    assert!(!ctx.is_granted(Capability::NetHost));
}

#[test]
fn events_round_trip_with_snake_case_tags() {
    let events = [
        AppEvent::Init,
        AppEvent::Resize {
            width: 1,
            height: 2,
            dpr: 1.5,
        },
        AppEvent::UiAction { id: "save".into() },
        AppEvent::StorageChanged {
            backend: StorageBackend::Node {
                node_id: "n".into(),
            },
            key: "k".into(),
        },
        AppEvent::Frame { dt_ms: 16 },
        AppEvent::Suspend {
            reason: SuspendReason::Backgrounded,
        },
        AppEvent::Resume,
    ];
    for e in events {
        assert_eq!(decode::<AppEvent>(&encode(&e)).unwrap(), e);
    }
    assert_eq!(encode(&AppEvent::Init), b"\x64init");
    let ui = encode(&AppEvent::UiAction { id: "x".into() });
    assert_eq!(ui, b"\xa1\x69ui_action\xa1\x62id\x61x");
    assert_eq!(encode(&AppEvent::Resume), b"\x66resume");
    let hidden = encode(&AppEvent::Suspend {
        reason: SuspendReason::Hidden,
    });
    assert_eq!(hidden, b"\xa1\x67suspend\xa1\x66reason\x66hidden");
}
