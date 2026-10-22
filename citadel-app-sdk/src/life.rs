//! The lifecycle as the guest SDK runs it: which events an app is given in each state, the step
//! each lifecycle event takes, and `app_close`. The table itself is
//! [`citadel_app_sdk_types::lifecycle`]'s, shared with every host.

use crate::abi::RawAbi;
use crate::rt::{settle, Slot};
use crate::{App, CloseContext};
use citadel_app_sdk_types::abi::CloseResult;
use citadel_app_sdk_types::{AppEvent, CloseReason, LifecycleState, LifecycleStep};

/// What becomes of an event in a state.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Admit {
    Deliver,
    /// A lifecycle event: delivered if this step is legal from here.
    Step(LifecycleStep),
    /// Not for an app in this state: frames and input only while running, replies until closed,
    /// nothing else once closing began.
    Drop,
}

pub(crate) fn admit(state: LifecycleState, event: &AppEvent) -> Admit {
    use LifecycleState as S;
    let alive = matches!(state, S::Initialised | S::Running | S::Suspended);
    let ok = |yes: bool| if yes { Admit::Deliver } else { Admit::Drop };
    match event {
        AppEvent::Init => Admit::Step(LifecycleStep::Start),
        AppEvent::Suspend { .. } => Admit::Step(LifecycleStep::Suspend),
        AppEvent::Resume => Admit::Step(LifecycleStep::Resume),
        AppEvent::ReplyReady { .. } => ok(alive || state == S::Closing),
        AppEvent::Frame { .. }
        | AppEvent::Pointer(_)
        | AppEvent::Key(_)
        | AppEvent::Wheel(_)
        | AppEvent::Focus { .. } => ok(state == S::Running),
        _ => ok(alive),
    }
}

/// `app_close`'s answer. The app's `close` runs once, from a state that may close; the replies
/// it waits on then reach it through `app_event` (`ReplyReady` is admitted while closing).
pub(crate) fn close_with<A: App>(slot: &Slot<A>, raw: &(impl RawAbi + 'static), code: i32) -> i32 {
    let Some(reason) = CloseReason::from_code(code) else {
        return CloseResult::UnknownReason.code();
    };
    let Ok(mut cell) = slot.app.try_borrow_mut() else {
        return CloseResult::NotRunning.code();
    };
    let Some(app) = cell.as_mut() else {
        return CloseResult::NotRunning.code();
    };
    let Ok(closing) = slot.life.get().step(LifecycleStep::Close) else {
        return CloseResult::NotRunning.code();
    };
    slot.life.set(closing);
    let ctx = CloseContext {
        grace_ms: slot.grace_ms.get(),
    };
    app.close(&ctx, reason);
    settle(raw, app);
    CloseResult::Ok.code()
}

#[cfg(test)]
#[path = "life_tests.rs"]
mod tests;
