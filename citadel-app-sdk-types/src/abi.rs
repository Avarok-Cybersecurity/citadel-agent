//! Names and numbers of the boundary itself: the import namespace, the export names, the
//! negative codes a group import or reply function may return, and `app_init`'s results.
//!
//! # Buffers and handles
//!
//! - The host writes an `AppContext` / `AppEvent` into a buffer it obtained from `app_alloc`,
//!   calls `app_init` / `app_event`, and frees the buffer with `app_free` once the call returns.
//!   The app never keeps a pointer into it. `app_alloc` answers 0 when it cannot allocate.
//!   Pointers are the guest's u32 values (a host reads them `>>> 0`).
//! - A group call answers a reply handle (>= 0) or an [`AbiError`] code (< 0). A handle is
//!   either ready at once (refusals, `ungranted`, size and rate limits, `canvas`, `log`) or
//!   PENDING: `reply_len` and `reply_read` answer [`AbiError::Pending`] until it resolves, and the
//!   host then delivers `AppEvent::ReplyReady { handle }` through `app_event`. A browser worker
//!   cannot block on a port, so no call except `canvas` and `log` may be assumed immediate.
//! - `reply_len` answers the length of a ready reply. `reply_read(handle, ptr, cap)` is one-shot:
//!   with `cap >= len` it copies the whole reply, answers its length and releases the handle;
//!   with `cap < len` it copies nothing, answers [`AbiError::BufferTooSmall`] and keeps it.
//! - An unknown op, arguments that are not the op's CBOR, and an oversized request are REPLIES
//!   (`{refused: {policy: "unknown_op"}}`, `{policy: "bad_arguments"}`, `{limit: "request_bytes"}`),
//!   not codes: only misuse of memory or handles is a code.

/// The only import module a Citadel app may name.
pub const NAMESPACE: &str = "citadel_app_abi_1";
/// The ABI identifier a manifest's `sdk.abi` names.
pub const ABI_ID: &str = "citadel-app-abi-1";
/// The memory import's field name. The module must import it, never define it.
pub const MEMORY: &str = "memory";
/// `(handle: i32) -> i32`: the reply's length, or an [`AbiError`].
pub const REPLY_LEN: &str = "reply_len";
/// `(handle: i32, ptr: i32, cap: i32) -> i32`: bytes copied, or an [`AbiError`].
pub const REPLY_READ: &str = "reply_read";

/// `(len: i32) -> i32`: a pointer to `len` fresh bytes, or 0.
pub const EXPORT_ALLOC: &str = "app_alloc";
/// `(ptr: i32, len: i32)`: release what `app_alloc` returned.
pub const EXPORT_FREE: &str = "app_free";
/// `(ptr: i32, len: i32) -> i32`: one CBOR `AppContext`; answers an [`InitResult`] code.
pub const EXPORT_INIT: &str = "app_init";
/// `(ptr: i32, len: i32)`: one CBOR `AppEvent`.
pub const EXPORT_EVENT: &str = "app_event";
/// `(dt_ms: i32)`: optional; one animation frame.
pub const EXPORT_FRAME: &str = "app_frame";
/// `(reason: i32) -> i32`: the graceful end. `reason` is a `CloseReason::code`; answers a
/// [`CloseResult`] code. REQUIRED (see the lifecycle doc): every app has a close path, so a host
/// never has to guess whether ending one loses its state.
pub const EXPORT_CLOSE: &str = "app_close";
pub const REQUIRED_EXPORTS: [&str; 5] = [
    EXPORT_ALLOC,
    EXPORT_FREE,
    EXPORT_INIT,
    EXPORT_EVENT,
    EXPORT_CLOSE,
];
pub const OPTIONAL_EXPORTS: [&str; 1] = [EXPORT_FRAME];

/// Bytes in one wasm page; memory limits are counted in pages.
pub const PAGE_BYTES: u32 = 65_536;

/// What a group import or reply function answers instead of a handle or a length. Misuse of
/// memory or handles, or a reply still pending: everything else is a reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum AbiError {
    /// `ptr..ptr+len` (or the `reply_read` target) is not inside the module's memory.
    BadRequest = -1,
    /// The handle was never issued, or was already read.
    UnknownHandle = -2,
    /// The reply has not arrived; `AppEvent::ReplyReady { handle }` follows when it does.
    Pending = -3,
    /// `reply_read`'s `cap` is smaller than the reply; nothing was copied, the handle stays live.
    BufferTooSmall = -4,
    /// Too many replies issued and not yet read.
    HandlesExhausted = -5,
}

impl AbiError {
    pub const ALL: [AbiError; 5] = [
        Self::BadRequest,
        Self::UnknownHandle,
        Self::Pending,
        Self::BufferTooSmall,
        Self::HandlesExhausted,
    ];

    pub const fn code(self) -> i32 {
        self as i32
    }

    pub fn from_code(code: i32) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.code() == code)
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::BadRequest => "BAD_REQUEST",
            Self::UnknownHandle => "UNKNOWN_HANDLE",
            Self::Pending => "PENDING",
            Self::BufferTooSmall => "BUFFER_TOO_SMALL",
            Self::HandlesExhausted => "HANDLES_EXHAUSTED",
        }
    }
}

/// The policy reason an unknown op is refused with.
pub const REFUSED_UNKNOWN_OP: &str = "unknown_op";
/// The policy reason arguments that are not the op's CBOR struct are refused with.
pub const REFUSED_BAD_ARGUMENTS: &str = "bad_arguments";
/// The limit a request larger than the host's request cap is refused with.
pub const REFUSED_REQUEST_BYTES: &str = "request_bytes";

/// The refusal (`{unavailable: ...}`) of a call a CLOSING app makes outside `CLOSING_GROUPS`.
pub const REFUSED_CLOSING: &str = "closing";
/// The refusal of every call once the instance is CLOSED.
pub const REFUSED_CLOSED: &str = "closed";

/// The platform's ceiling on a manifest's `limits.close_grace_ms`: the longest `app_close` and
/// the replies it waits on may take before the host ends the worker. A bounded-cleanup contract
/// the app is told (`AppContext::close_grace_ms`), not a timeout that papers over a hang.
pub const MAX_CLOSE_GRACE_MS: u32 = 2000;

/// Internal HTTP (R4.4): body chunks are at most this many bytes.
pub const IHTTP_MAX_CHUNK_BYTES: u32 = 65_536;
/// Internal HTTP (R4.4): body frames each side may send before the other grants credit. Implicit:
/// neither side sends `Credit { n: 4 }` first.
pub const IHTTP_INITIAL_CREDIT: u32 = 4;

/// What `app_init` answers. Any other non-zero value is app-defined and means it did not start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum InitResult {
    Ok = 0,
    /// The buffer was not a CBOR `AppContext`.
    BadContext = 1,
    /// The app's own `init` declined to start.
    Declined = 2,
}

impl InitResult {
    pub const ALL: [InitResult; 3] = [Self::Ok, Self::BadContext, Self::Declined];

    pub const fn code(self) -> i32 {
        self as i32
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::BadContext => "BAD_CONTEXT",
            Self::Declined => "DECLINED",
        }
    }
}

/// What `app_close` answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum CloseResult {
    /// The app's close ran.
    Ok = 0,
    /// `reason` is not a `CloseReason` code; nothing ran.
    UnknownReason = 1,
    /// There is no running app to close (init never succeeded, or it is already closing or
    /// closed); nothing ran. A second `app_close` answers this.
    NotRunning = 2,
}

impl CloseResult {
    pub const ALL: [CloseResult; 3] = [Self::Ok, Self::UnknownReason, Self::NotRunning];

    pub const fn code(self) -> i32 {
        self as i32
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::UnknownReason => "UNKNOWN_REASON",
            Self::NotRunning => "NOT_RUNNING",
        }
    }
}
