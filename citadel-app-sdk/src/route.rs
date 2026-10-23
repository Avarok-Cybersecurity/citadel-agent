//! Where an admitted event goes: a lifecycle method, the SDK's own machinery (replies,
//! internal-HTTP frames, topic events, fan-out clocks), or the app's `event`.

use crate::abi::RawAbi;
use crate::{ihttp, pending, topic, App};
use citadel_app_sdk_types::AppEvent;

pub(crate) fn deliver<A: App>(raw: &(impl RawAbi + 'static), app: &mut A, event: AppEvent) {
    match event {
        AppEvent::Suspend { reason } => app.suspend(reason),
        AppEvent::Resume => app.resume(),
        AppEvent::ReplyReady { handle } if pending::reply_ready(raw, handle) => {}
        AppEvent::Ihttp { from, frame } => {
            if let Some((from, frame)) = ihttp::route(raw, from, frame, app) {
                app.event(AppEvent::Ihttp { from, frame });
            }
        }
        AppEvent::Topic { name, event } => {
            if let Some((name, event)) = topic::route(app, name, event) {
                app.event(AppEvent::Topic { name, event });
            }
        }
        AppEvent::Frame { dt_ms } => {
            ihttp::fanout_tick(dt_ms, app);
            app.frame(dt_ms)
        }
        other => app.event(other),
    }
}
