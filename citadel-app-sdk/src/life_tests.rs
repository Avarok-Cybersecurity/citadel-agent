use super::*;
use crate::abi::tests::Fake;
use crate::rt::tests::ctx;
use crate::rt::{event_with, init_with, Delivery, Slot};
use crate::{InitFailure, SuspendReason};
use citadel_app_sdk_types::calls::KvSet;
use citadel_app_sdk_types::lifecycle::IllegalTransition;
use citadel_app_sdk_types::{encode, AppContext, Bytes, HostReply};

thread_local! {
    static HOST: Fake = Fake::default();
}

/// Records every lifecycle call; its close writes its state and notes when the write landed.
#[derive(Debug, Default)]
struct Life {
    seen: Vec<String>,
}

impl App for Life {
    fn init(_ctx: AppContext) -> Result<Self, InitFailure> {
        Ok(Life::default())
    }
    fn event(&mut self, event: AppEvent) {
        self.seen.push(format!("event {event:?}"));
    }
    fn frame(&mut self, _dt_ms: u32) {
        self.seen.push("frame".into());
    }
    fn suspend(&mut self, reason: SuspendReason) {
        self.seen.push(format!("suspend {reason:?}"));
    }
    fn resume(&mut self) {
        self.seen.push("resume".into());
    }
    fn close(&mut self, ctx: &CloseContext, reason: CloseReason) {
        self.seen
            .push(format!("close {reason:?} {}ms", ctx.grace_ms));
        HOST.with(|h| {
            let write = KvSet {
                backend: citadel_app_sdk_types::calls::KvBackend::User,
                key: "state".into(),
                value: Bytes(b"saved".to_vec()),
            };
            crate::pending::start(h, &write)
                .then(|app: &mut Life, r: Result<(), _>| app.seen.push(format!("flushed {r:?}")));
        });
    }
}

fn started() -> Slot<Life> {
    let slot = Slot::<Life>::new();
    HOST.with(|h| {
        assert_eq!(init_with(&slot, h, &ctx("en")), 0);
        assert_eq!(event_with(&slot, h, AppEvent::Init), Delivery::Delivered);
    });
    assert_eq!(slot.lifecycle(), LifecycleState::Running);
    slot
}

fn seen(slot: &Slot<Life>) -> Vec<String> {
    slot.app.borrow().as_ref().unwrap().seen.clone()
}

#[test]
fn suspend_and_resume_reach_the_app_and_frames_stop_between_them() {
    let slot = started();
    HOST.with(|h| {
        let hidden = AppEvent::Suspend {
            reason: SuspendReason::Hidden,
        };
        assert_eq!(event_with(&slot, h, hidden.clone()), Delivery::Delivered);
        assert_eq!(slot.lifecycle(), LifecycleState::Suspended);
        let frame = AppEvent::Frame { dt_ms: 16 };
        assert_eq!(
            event_with(&slot, h, frame.clone()),
            Delivery::Dropped(LifecycleState::Suspended)
        );
        assert_eq!(
            event_with(&slot, h, hidden),
            Delivery::Illegal(IllegalTransition {
                from: LifecycleState::Suspended,
                step: LifecycleStep::Suspend
            })
        );
        assert_eq!(event_with(&slot, h, AppEvent::Resume), Delivery::Delivered);
        assert_eq!(event_with(&slot, h, frame), Delivery::Delivered);
    });
    assert_eq!(
        seen(&slot),
        ["event Init", "suspend Hidden", "resume", "frame"]
    );
}

#[test]
fn a_resume_of_a_running_app_and_a_second_start_are_refused() {
    let slot = started();
    HOST.with(|h| {
        assert!(matches!(
            event_with(&slot, h, AppEvent::Resume),
            Delivery::Illegal(_)
        ));
        assert!(matches!(
            event_with(&slot, h, AppEvent::Init),
            Delivery::Illegal(_)
        ));
    });
    assert_eq!(seen(&slot), ["event Init"]);
}

#[test]
fn close_runs_once_with_its_grace_and_the_reply_it_waits_on_still_arrives() {
    let slot = started();
    HOST.with(|h| {
        let code = CloseReason::Uninstall.code();
        assert_eq!(close_with(&slot, h, code), CloseResult::Ok.code());
        assert_eq!(slot.lifecycle(), LifecycleState::Closing);
        assert_eq!(close_with(&slot, h, code), CloseResult::NotRunning.code());
        // Input is over; the write's reply is not.
        let key = AppEvent::Focus { focused: true };
        assert_eq!(
            event_with(&slot, h, key),
            Delivery::Dropped(LifecycleState::Closing)
        );
        let handle = *h.replies.borrow().keys().last().unwrap();
        h.resolve(handle, encode(&HostReply::Ok(())));
        let ready = AppEvent::ReplyReady { handle };
        assert_eq!(event_with(&slot, h, ready), Delivery::Delivered);
    });
    assert_eq!(
        seen(&slot),
        ["event Init", "close Uninstall 250ms", "flushed Ok(())"]
    );
}

#[test]
fn an_unknown_reason_and_an_app_never_started_close_nothing() {
    HOST.with(|h| {
        let slot = started();
        assert_eq!(close_with(&slot, h, 99), CloseResult::UnknownReason.code());
        assert_eq!(slot.lifecycle(), LifecycleState::Running);
        let never = Slot::<Life>::new();
        assert_eq!(close_with(&never, h, 0), CloseResult::NotRunning.code());
        assert_eq!(never.lifecycle(), LifecycleState::Created);
    });
}

#[test]
fn an_initialised_app_may_close_before_it_starts() {
    let slot = Slot::<Life>::new();
    HOST.with(|h| {
        assert_eq!(init_with(&slot, h, &ctx("en")), 0);
        let frame = AppEvent::Frame { dt_ms: 16 };
        assert_eq!(
            event_with(&slot, h, frame),
            Delivery::Dropped(LifecycleState::Initialised)
        );
        assert_eq!(close_with(&slot, h, 0), CloseResult::Ok.code());
    });
    assert_eq!(seen(&slot), ["close Unmount 250ms"]);
}

#[test]
fn what_each_state_admits() {
    use LifecycleState as S;
    let pointer = AppEvent::Wheel(citadel_app_sdk_types::WheelEvent {
        x: 0.0,
        y: 0.0,
        dx: 0.0,
        dy: 1.0,
        modifiers: citadel_app_sdk_types::Modifiers {
            shift: false,
            ctrl: false,
            alt: false,
            meta: false,
        },
    });
    let node = AppEvent::NodeChanged {
        node_id: "n".into(),
    };
    let reply = AppEvent::ReplyReady { handle: 1 };
    let rows = [
        (
            S::Initialised,
            [Admit::Drop, Admit::Deliver, Admit::Deliver],
        ),
        (S::Running, [Admit::Deliver, Admit::Deliver, Admit::Deliver]),
        (S::Suspended, [Admit::Drop, Admit::Deliver, Admit::Deliver]),
        (S::Closing, [Admit::Drop, Admit::Drop, Admit::Deliver]),
        (S::Closed, [Admit::Drop, Admit::Drop, Admit::Drop]),
    ];
    for (state, want) in rows {
        let got = [&pointer, &node, &reply].map(|e| admit(state, e));
        assert_eq!(got, want, "{state:?}");
    }
}
