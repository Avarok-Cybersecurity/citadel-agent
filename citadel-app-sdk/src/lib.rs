//! Write a Citadel app: implement [`App`], name it in [`export_app!`], build for
//! `wasm32-unknown-unknown`, and link it the ABI's way (`.cargo/config.toml`):
//!
//! ```text
//! [target.wasm32-unknown-unknown]
//! rustflags = [
//!     "-C", "link-arg=--import-memory=citadel_app_abi_1,memory",
//!     "-C", "link-arg=--no-check-features",
//!     "-C", "link-arg=--features=mutable-globals,sign-ext,nontrapping-fptoint,multivalue,reference-types",
//! ]
//! ```
//!
//! The memory is imported so the host sets its hard maximum. The features line keeps wasm-ld
//! from adding a start function to zero `.bss` in the imported memory (it writes `.bss` as data
//! instead); a module with a start function is refused.
//!
//! The module then reaches nothing but its host, through the groups in [`types::Group`].
//! `canvas` and `log` answer at once (`Result<_, Refused>`); every other call answers a
//! [`Pending`], because a browser worker cannot block on a port: attach [`Pending::then`] (it
//! runs with the app once the reply is in) or keep it and take it on `AppEvent::ReplyReady`.
//! A refusal, `ungranted` included, is ready at once ([`Pending::take_ready`]). A panic is
//! logged through the `log` group and traps: nothing unwinds across the boundary.
#[cfg(not(any(target_arch = "wasm32", test)))]
compile_error!("citadel-app-sdk builds for wasm32-unknown-unknown only (cargo build --target wasm32-unknown-unknown)");

mod abi;
#[cfg(target_arch = "wasm32")]
mod api;
#[cfg(target_arch = "wasm32")]
mod ffi;
pub mod ihttp;
mod life;
mod pending;
#[doc(hidden)]
pub mod rt;

#[cfg(target_arch = "wasm32")]
pub use api::*;
pub use citadel_app_sdk_types as types;
pub use citadel_app_sdk_types::{AppContext, AppEvent, CloseReason, Refused, SuspendReason};
#[cfg(target_arch = "wasm32")]
pub use ffi::{call, call_now};
pub use pending::Pending;

/// Why `init` did not start the app: the code `app_init` answers. 0 and 1 are the ABI's own
/// (`OK`, `BAD_CONTEXT`) and are answered as [`InitFailure::DECLINED`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InitFailure(pub i32);

impl InitFailure {
    pub const DECLINED: InitFailure = InitFailure(types::abi::InitResult::Declined.code());
}

/// How long a closing app has: the install's `close_grace_ms` (`AppContext::close_grace_ms`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CloseContext {
    /// From the moment `app_close` is called until the host ends the instance, whether or not
    /// `close` returned and its replies arrived. A bound on cleanup, not a timeout to retry in.
    pub grace_ms: u32,
}

/// An app. One instance per `app_init`; the host calls it from one thread, never re-entrantly.
///
/// Its life is [`types::lifecycle`]'s state machine: `init` -> started by `AppEvent::Init` ->
/// `suspend`/`resume` while its surface is out of sight -> `close`, exactly once. Every method
/// is required except `frame`, so an app states what each step means to it.
pub trait App: Sized + 'static {
    fn init(ctx: AppContext) -> Result<Self, InitFailure>;
    fn event(&mut self, event: AppEvent);
    /// One animation frame (`app_frame`, or `AppEvent::Frame`), only while running.
    fn frame(&mut self, _dt_ms: u32) {}
    /// The surface is out of sight: no frames or input arrive until [`App::resume`]. Replies,
    /// storage changes and internal-HTTP frames still do.
    fn suspend(&mut self, reason: SuspendReason);
    fn resume(&mut self);
    /// The graceful end, called once. Persist what must survive through `storage` (the only
    /// group besides `log` a closing app may call); attach `Pending::then` to the writes, and
    /// the host delivers their replies before it ends the instance, within `ctx.grace_ms`.
    /// Nothing of the app runs after that.
    fn close(&mut self, ctx: &CloseContext, reason: CloseReason);
}

/// Export `app_alloc`, `app_free`, `app_init`, `app_event`, `app_frame` and `app_close` for
/// `$app`: an app built with it cannot lack the close path the ABI requires.
#[macro_export]
macro_rules! export_app {
    ($app:ty) => {
        static __CITADEL_APP: $crate::rt::Slot<$app> = $crate::rt::Slot::new();

        #[no_mangle]
        pub extern "C" fn app_alloc(len: i32) -> i32 {
            $crate::rt::wasm_alloc(len)
        }

        /// # Safety
        /// `ptr`/`len` are what `app_alloc` returned and was asked for.
        #[no_mangle]
        pub unsafe extern "C" fn app_free(ptr: i32, len: i32) {
            unsafe { $crate::rt::wasm_free(ptr, len) }
        }

        /// # Safety
        /// `ptr..ptr+len` is a live `app_alloc` buffer.
        #[no_mangle]
        pub unsafe extern "C" fn app_init(ptr: i32, len: i32) -> i32 {
            unsafe { $crate::rt::wasm_init(&__CITADEL_APP, ptr, len) }
        }

        /// # Safety
        /// `ptr..ptr+len` is a live `app_alloc` buffer.
        #[no_mangle]
        pub unsafe extern "C" fn app_event(ptr: i32, len: i32) {
            unsafe { $crate::rt::wasm_event(&__CITADEL_APP, ptr, len) }
        }

        #[no_mangle]
        pub extern "C" fn app_frame(dt_ms: i32) {
            $crate::rt::wasm_frame(&__CITADEL_APP, dt_ms)
        }

        #[no_mangle]
        pub extern "C" fn app_close(reason: i32) -> i32 {
            $crate::rt::wasm_close(&__CITADEL_APP, reason)
        }
    };
}
