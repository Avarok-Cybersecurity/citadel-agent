//! One host call, independent of how the host is reached: invoke the group, then read the reply
//! now or learn that it is pending. Pure over [`RawAbi`], so it is tested natively with a fake.

use citadel_app_sdk_types::abi::AbiError;
use citadel_app_sdk_types::{decode, Group, HostReply, Refused};
use serde::de::DeserializeOwned;

/// The three imports every call uses.
pub(crate) trait RawAbi {
    fn invoke(&self, group: Group, op: i32, payload: &[u8]) -> i32;
    fn reply_len(&self, handle: i32) -> i32;
    fn reply_read(&self, handle: i32, buf: &mut [u8]) -> i32;
}

/// Where a call stands.
#[derive(Debug, PartialEq)]
pub(crate) enum Sent {
    /// The reply bytes, or why there are none.
    Done(Result<Vec<u8>, Refused>),
    /// The handle resolves later; `AppEvent::ReplyReady { handle }` says when.
    Pending(i32),
}

/// What a negative ABI code means to the app.
pub(crate) fn refusal(code: i32) -> Refused {
    match AbiError::from_code(code) {
        Some(AbiError::BufferTooSmall) => Refused::Limit("reply".into()),
        Some(AbiError::HandlesExhausted) => Refused::Limit("handles".into()),
        Some(e) => Refused::Unavailable(e.name().into()),
        None => Refused::Unavailable(format!("abi error {code}")),
    }
}

/// Inlined into each op's call site, so the group is a constant there and the linker imports
/// only the groups an app actually calls.
#[inline(always)]
pub(crate) fn send(raw: &impl RawAbi, group: Group, op: i32, payload: &[u8]) -> Sent {
    let handle = raw.invoke(group, op, payload);
    if handle < 0 {
        return Sent::Done(Err(refusal(handle)));
    }
    read(raw, handle)
}

/// Read `handle` if it is ready. A reply larger than the app can allocate is `Limit("reply")`,
/// never an allocation failure.
pub(crate) fn read(raw: &impl RawAbi, handle: i32) -> Sent {
    let pending = AbiError::Pending.code();
    let len = raw.reply_len(handle);
    if len == pending {
        return Sent::Pending(handle);
    }
    if len < 0 {
        return Sent::Done(Err(refusal(len)));
    }
    let len = len as usize;
    let mut buf = Vec::new();
    if buf.try_reserve_exact(len).is_err() {
        return Sent::Done(Err(Refused::Limit("reply".into())));
    }
    buf.resize(len, 0);
    let got = raw.reply_read(handle, &mut buf);
    if got == pending {
        return Sent::Pending(handle);
    }
    if got < 0 {
        return Sent::Done(Err(refusal(got)));
    }
    if got as usize != len {
        return Sent::Done(Err(Refused::Unavailable("short reply".into())));
    }
    Sent::Done(Ok(buf))
}

/// The `ok` payload of a reply, or its refusal.
pub(crate) fn decode_reply<R: DeserializeOwned>(bytes: &[u8]) -> Result<R, Refused> {
    decode::<HostReply<R>>(bytes)
        .map_err(|_| Refused::Unavailable("malformed reply".into()))?
        .into_result()
}

#[cfg(test)]
#[path = "abi_tests.rs"]
pub(crate) mod tests;
