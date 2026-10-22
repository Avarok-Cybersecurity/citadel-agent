use super::*;
use crate::abi::tests::Fake;
use citadel_app_sdk_types::{encode, AppScope, Capability, Size, Surface};

#[derive(Debug)]
struct Recorder {
    ctx: AppContext,
    seen: Vec<AppEvent>,
    frames: Vec<u32>,
}

impl App for Recorder {
    fn init(ctx: AppContext) -> Result<Self, InitFailure> {
        match ctx.locale.as_str() {
            "decline" => Err(InitFailure(1)),
            "code" => Err(InitFailure(107)),
            _ => Ok(Recorder {
                ctx,
                seen: vec![],
                frames: vec![],
            }),
        }
    }
    fn event(&mut self, event: AppEvent) {
        self.seen.push(event);
    }
    fn frame(&mut self, dt_ms: u32) {
        self.frames.push(dt_ms);
    }
}

fn ctx(locale: &str) -> Vec<u8> {
    encode(&AppContext {
        app_id: "a.b".into(),
        install_id: "i".into(),
        scope: AppScope::Workspace,
        granted: vec![Capability::Canvas],
        surface: Surface::CanvasPane,
        size: Size {
            width: 1,
            height: 1,
        },
        dpr: 1.0,
        theme: Default::default(),
        locale: locale.into(),
        acting_username: "u".into(),
        node_id: "n".into(),
    })
}

#[test]
fn init_answers_ok_once_and_declines_a_second_init() {
    let slot = Slot::<Recorder>::new();
    assert_eq!(init_with(&slot, &Fake::default(), &ctx("en")), 0);
    assert_eq!(slot.0.borrow().as_ref().unwrap().ctx.locale, "en");
    assert_eq!(init_with(&slot, &Fake::default(), &ctx("en")), 2);
}

#[test]
fn a_context_that_does_not_decode_is_bad_context() {
    let slot = Slot::<Recorder>::new();
    assert_eq!(init_with(&slot, &Fake::default(), &[0xff]), 1);
    assert_eq!(init_with(&slot, &Fake::default(), &[]), 1);
    assert!(slot.0.borrow().is_none());
}

#[test]
fn an_app_failure_keeps_its_code_but_never_claims_ok_or_bad_context() {
    assert_eq!(
        init_with(&Slot::<Recorder>::new(), &Fake::default(), &ctx("code")),
        107
    );
    assert_eq!(
        init_with(&Slot::<Recorder>::new(), &Fake::default(), &ctx("decline")),
        2
    );
}

#[test]
fn events_and_frames_reach_the_app_only_after_init() {
    let slot = Slot::<Recorder>::new();
    assert_eq!(
        event_with(&slot, &Fake::default(), AppEvent::Init),
        Delivery::NotStarted
    );
    init_with(&slot, &Fake::default(), &ctx("en"));
    assert_eq!(
        event_with(&slot, &Fake::default(), AppEvent::Init),
        Delivery::Delivered
    );
    assert_eq!(
        event_with(&slot, &Fake::default(), AppEvent::Frame { dt_ms: 16 }),
        Delivery::Delivered
    );
    frame_with(&slot, &Fake::default(), -5);
    let app = slot.0.borrow();
    let app = app.as_ref().unwrap();
    assert_eq!(app.seen, [AppEvent::Init]);
    assert_eq!(app.frames, [16, 0]);
}

#[test]
fn a_reentrant_event_is_refused_not_a_double_borrow() {
    let slot = Slot::<Recorder>::new();
    init_with(&slot, &Fake::default(), &ctx("en"));
    let _held = slot.0.borrow_mut();
    assert_eq!(
        event_with(&slot, &Fake::default(), AppEvent::Init),
        Delivery::Busy
    );
}

#[test]
fn alloc_refuses_non_positive_lengths_and_round_trips_with_free() {
    assert_eq!(alloc_bytes(0), None);
    assert_eq!(alloc_bytes(-1), None);
    let p = alloc_bytes(64).unwrap();
    // SAFETY: freshly allocated with this length.
    unsafe {
        p.write_bytes(0xab, 64);
        free_bytes(p, 64);
        free_bytes(core::ptr::null_mut(), 64);
    }
}

thread_local! {
    static HOST: Fake = Fake::default();
}

struct Asker {
    names: Vec<String>,
    seen: Vec<AppEvent>,
}

impl App for Asker {
    fn init(_ctx: AppContext) -> Result<Self, InitFailure> {
        use citadel_app_sdk_types::calls::NodeGet;
        HOST.with(|h| {
            crate::pending::start(
                h,
                &NodeGet {
                    node_id: "n".into(),
                },
            )
            .map(|n| n.name)
            .then(|app: &mut Asker, r| app.names.push(r.unwrap()))
        });
        Ok(Asker {
            names: vec![],
            seen: vec![],
        })
    }
    fn event(&mut self, event: AppEvent) {
        self.seen.push(event);
    }
}

#[test]
fn reply_ready_for_an_sdk_handle_runs_its_continuation_and_any_other_reaches_the_app() {
    use citadel_app_sdk_types::calls::NodeInfo;
    use citadel_app_sdk_types::HostReply;
    HOST.with(|h| {
        let slot = Slot::<Asker>::new();
        assert_eq!(init_with(&slot, h, &ctx("en")), 0);
        let info = NodeInfo {
            node_id: "n".into(),
            name: "late".into(),
            parent_id: None,
            has_children: false,
        };
        h.resolve(1, encode(&HostReply::Ok(info)));
        assert_eq!(
            event_with(&slot, h, AppEvent::ReplyReady { handle: 1 }),
            Delivery::Delivered
        );
        assert_eq!(slot.0.borrow().as_ref().unwrap().names, ["late"]);
        assert!(
            slot.0.borrow().as_ref().unwrap().seen.is_empty(),
            "the SDK's own reply is not the app's event"
        );
        event_with(&slot, h, AppEvent::ReplyReady { handle: 99 });
        assert_eq!(
            slot.0.borrow().as_ref().unwrap().seen,
            [AppEvent::ReplyReady { handle: 99 }]
        );
    });
}

#[test]
fn a_reply_that_came_back_during_init_is_handled_before_app_init_returns() {
    use citadel_app_sdk_types::HostReply;
    HOST.with(|h| {
        h.queue
            .borrow_mut()
            .push_back(encode(&HostReply::<()>::Refused(
                citadel_app_sdk_types::Refused::Ungranted,
            )));
        let slot = Slot::<Asker>::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            init_with(&slot, h, &ctx("en"))
        }));
        // The continuation unwraps, so an ungranted answer panics inside init's settle: proof it ran there.
        assert!(
            result.is_err(),
            "the continuation ran before app_init returned"
        );
    });
}
