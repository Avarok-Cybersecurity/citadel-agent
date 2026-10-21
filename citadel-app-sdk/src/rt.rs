//! The exports' machinery: the alloc protocol, the app's slot, and init/event/frame dispatch.
//! The pure parts take bytes and an [`App`]; only the `wasm_*` entry points touch raw pointers
//! and the host.

use crate::abi::RawAbi;
use crate::{ihttp, pending, App, InitFailure};
use citadel_app_sdk_types::abi::InitResult;
use citadel_app_sdk_types::calls::{LogLevel, LogWrite};
use citadel_app_sdk_types::{decode, AppContext, AppEvent};
use core::cell::RefCell;
use std::alloc::{alloc, dealloc, Layout};

/// Where the one app instance lives between calls.
pub struct Slot<A>(RefCell<Option<A>>);

impl<A> Slot<A> {
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Self {
        Slot(RefCell::new(None))
    }
}

// SAFETY: an app is single-threaded. The ABI admits no shared memory and no atomics (the
// inspector refuses both), so no second thread exists to observe the cell.
#[cfg(target_arch = "wasm32")]
unsafe impl<A> Sync for Slot<A> {}

fn layout(len: i32) -> Option<Layout> {
    if len <= 0 {
        return None;
    }
    Layout::from_size_align(len as usize, 1).ok()
}

/// `len` fresh bytes, or `None` (len <= 0, or the memory cannot grow).
pub fn alloc_bytes(len: i32) -> Option<*mut u8> {
    let layout = layout(len)?;
    // SAFETY: `layout` has a non-zero size.
    let ptr = unsafe { alloc(layout) };
    (!ptr.is_null()).then_some(ptr)
}

/// # Safety
/// `ptr` came from [`alloc_bytes`] with this `len` and is not used again.
pub unsafe fn free_bytes(ptr: *mut u8, len: i32) {
    if let (false, Some(layout)) = (ptr.is_null(), layout(len)) {
        // SAFETY: the caller's contract.
        unsafe { dealloc(ptr, layout) }
    }
}

/// Run what became ready, then log what the SDK's own background calls noted.
fn settle(raw: &(impl RawAbi + 'static), app: &mut dyn std::any::Any) {
    pending::drain(app);
    for message in pending::take_notes() {
        let _ = pending::now(
            raw,
            &LogWrite {
                level: LogLevel::Warn,
                message,
            },
        );
    }
}

/// `app_init`'s answer for this context.
pub(crate) fn init_with<A: App>(
    slot: &Slot<A>,
    raw: &(impl RawAbi + 'static),
    bytes: &[u8],
) -> i32 {
    let Ok(ctx) = decode::<AppContext>(bytes) else {
        return InitResult::BadContext.code();
    };
    let Ok(mut cell) = slot.0.try_borrow_mut() else {
        return InitFailure::DECLINED.0;
    };
    if cell.is_some() {
        return InitFailure::DECLINED.0;
    }
    match A::init(ctx) {
        Ok(app) => {
            settle(raw, cell.insert(app));
            InitResult::Ok.code()
        }
        Err(InitFailure(code))
            if code == InitResult::Ok.code() || code == InitResult::BadContext.code() =>
        {
            InitFailure::DECLINED.0
        }
        Err(InitFailure(code)) => code,
    }
}

/// What became of an event.
#[derive(Debug, PartialEq, Eq)]
pub enum Delivery {
    Delivered,
    /// `app_init` has not succeeded.
    NotStarted,
    /// The host re-entered the app during one of its own calls.
    Busy,
}

pub(crate) fn event_with<A: App>(
    slot: &Slot<A>,
    raw: &(impl RawAbi + 'static),
    event: AppEvent,
) -> Delivery {
    let Ok(mut cell) = slot.0.try_borrow_mut() else {
        return Delivery::Busy;
    };
    let Some(app) = cell.as_mut() else {
        return Delivery::NotStarted;
    };
    match event {
        AppEvent::ReplyReady { handle } if pending::reply_ready(raw, handle) => {}
        AppEvent::Ihttp { from, frame } => {
            if let Some((from, frame)) = ihttp::route(raw, from, frame, app) {
                app.event(AppEvent::Ihttp { from, frame });
            }
        }
        AppEvent::Frame { dt_ms } => app.frame(dt_ms),
        other => app.event(other),
    }
    settle(raw, app);
    Delivery::Delivered
}

pub(crate) fn frame_with<A: App>(slot: &Slot<A>, raw: &(impl RawAbi + 'static), dt_ms: i32) {
    let _ = event_with(
        slot,
        raw,
        AppEvent::Frame {
            dt_ms: dt_ms.max(0) as u32,
        },
    );
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::*;
    use crate::ffi::WASM;
    use crate::log;

    pub fn wasm_alloc(len: i32) -> i32 {
        alloc_bytes(len).map_or(0, |p| p as usize as i32)
    }

    /// # Safety
    /// As [`free_bytes`].
    pub unsafe fn wasm_free(ptr: i32, len: i32) {
        unsafe { free_bytes(ptr as usize as *mut u8, len) }
    }

    /// # Safety
    /// `ptr..ptr+len` is a live buffer the host wrote.
    unsafe fn bytes<'a>(ptr: i32, len: i32) -> Option<&'a [u8]> {
        (ptr != 0 && len >= 0).then(|| unsafe {
            core::slice::from_raw_parts(ptr as usize as *const u8, len as usize)
        })
    }

    fn install_panic_hook() {
        std::panic::set_hook(Box::new(|info| {
            let _ = log::error(&info.to_string());
        }));
    }

    /// # Safety
    /// As [`bytes`].
    pub unsafe fn wasm_init<A: App>(slot: &Slot<A>, ptr: i32, len: i32) -> i32 {
        install_panic_hook();
        match unsafe { bytes(ptr, len) } {
            None => InitResult::BadContext.code(),
            Some(b) => init_with(slot, &WASM, b),
        }
    }

    /// # Safety
    /// As [`bytes`].
    pub unsafe fn wasm_event<A: App>(slot: &Slot<A>, ptr: i32, len: i32) {
        let Some(b) = (unsafe { bytes(ptr, len) }) else {
            let _ = log::warn("app_event without a buffer");
            return;
        };
        match decode::<AppEvent>(b) {
            Ok(event) => {
                if let delivery @ (Delivery::NotStarted | Delivery::Busy) =
                    event_with(slot, &WASM, event)
                {
                    let _ = log::warn(&format!("app_event dropped: {delivery:?}"));
                }
            }
            Err(e) => {
                let _ = log::warn(&format!("app_event: {e}"));
            }
        }
    }

    pub fn wasm_frame<A: App>(slot: &Slot<A>, dt_ms: i32) {
        frame_with(slot, &WASM, dt_ms)
    }
}

#[cfg(target_arch = "wasm32")]
pub use wasm::*;

#[cfg(test)]
#[path = "rt_tests.rs"]
mod tests;
