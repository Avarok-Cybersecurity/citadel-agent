//! The imports, and the one place they are called.

use crate::abi::RawAbi;
use crate::pending::{now, start, Pending};
use citadel_app_sdk_types::{Group, HostOp, Refused};

#[link(wasm_import_module = "citadel_app_abi_1")]
extern "C" {
    fn canvas(op: i32, ptr: i32, len: i32) -> i32;
    fn ui(op: i32, ptr: i32, len: i32) -> i32;
    fn events(op: i32, ptr: i32, len: i32) -> i32;
    fn navigate(op: i32, ptr: i32, len: i32) -> i32;
    fn nodes(op: i32, ptr: i32, len: i32) -> i32;
    fn md(op: i32, ptr: i32, len: i32) -> i32;
    fn files(op: i32, ptr: i32, len: i32) -> i32;
    fn chat(op: i32, ptr: i32, len: i32) -> i32;
    fn members(op: i32, ptr: i32, len: i32) -> i32;
    fn storage(op: i32, ptr: i32, len: i32) -> i32;
    fn net(op: i32, ptr: i32, len: i32) -> i32;
    fn ihttp(op: i32, ptr: i32, len: i32) -> i32;
    fn clipboard(op: i32, ptr: i32, len: i32) -> i32;
    fn notify(op: i32, ptr: i32, len: i32) -> i32;
    fn documents(op: i32, ptr: i32, len: i32) -> i32;
    fn log(op: i32, ptr: i32, len: i32) -> i32;
    fn topic(op: i32, ptr: i32, len: i32) -> i32;
    fn reply_len(handle: i32) -> i32;
    fn reply_read(handle: i32, ptr: i32, cap: i32) -> i32;
}

pub(crate) struct Wasm;

pub(crate) static WASM: Wasm = Wasm;

impl RawAbi for Wasm {
    #[inline(always)]
    fn invoke(&self, group: Group, op: i32, payload: &[u8]) -> i32 {
        let (p, l) = (payload.as_ptr() as usize as i32, payload.len() as i32);
        // SAFETY: the host only reads `l` bytes at `p`, a live slice for the whole call. Only
        // the linker resolves an import; the match is exhaustive, so every group has one.
        unsafe {
            match group {
                Group::Canvas => canvas(op, p, l),
                Group::Ui => ui(op, p, l),
                Group::Events => events(op, p, l),
                Group::Navigate => navigate(op, p, l),
                Group::Nodes => nodes(op, p, l),
                Group::Md => md(op, p, l),
                Group::Files => files(op, p, l),
                Group::Chat => chat(op, p, l),
                Group::Members => members(op, p, l),
                Group::Storage => storage(op, p, l),
                Group::Net => net(op, p, l),
                Group::Ihttp => ihttp(op, p, l),
                Group::Clipboard => clipboard(op, p, l),
                Group::Notify => notify(op, p, l),
                Group::Documents => documents(op, p, l),
                Group::Log => log(op, p, l),
                Group::Topic => topic(op, p, l),
            }
        }
    }

    fn reply_len(&self, handle: i32) -> i32 {
        // SAFETY: takes and returns plain integers.
        unsafe { reply_len(handle) }
    }

    fn reply_read(&self, handle: i32, buf: &mut [u8]) -> i32 {
        // SAFETY: the host writes at most `buf.len()` bytes into `buf`.
        unsafe { reply_read(handle, buf.as_mut_ptr() as usize as i32, buf.len() as i32) }
    }
}

/// Make any call; its reply may be pending. Every wrapper in the group modules is this with its
/// op filled in.
pub fn call<O: HostOp + 'static>(op: &O) -> Pending<O::Reply> {
    start(&WASM, op)
}

/// Make a call the host answers at once (`canvas`, `log`).
pub fn call_now<O: HostOp>(op: &O) -> Result<O::Reply, Refused> {
    now(&WASM, op)
}

impl<T: 'static> Pending<T> {
    /// The reply, once its `AppEvent::ReplyReady` has arrived (or it came back at once); `None`
    /// while it is still pending.
    pub fn try_take(&mut self) -> Option<Result<T, Refused>> {
        self.poll_with(&WASM)
    }
}
