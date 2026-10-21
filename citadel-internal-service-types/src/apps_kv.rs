//! A Citadel app's `local` storage: a key-value store on the member's own machine.
//!
//! The agent keeps it in the account's LocalDB, under a namespace it builds itself from the
//! app id, the install's scope key and the account's username. A request names a key inside its
//! namespace and nothing else: no key, however it is spelled, reaches another install's data or
//! a store the agent uses for anything else.

use custom_debug::Debug;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg(feature = "typescript")]
use ts_rs::TS;

/// The longest key, in bytes.
pub const APP_KV_MAX_KEY_BYTES: usize = 256;
/// The largest value, in bytes.
pub const APP_KV_MAX_VALUE_BYTES: usize = 256 * 1024;
/// What one app, scope and account may hold in all, entries' overhead included.
pub const APP_KV_MAX_TOTAL_BYTES: u64 = 4 * 1024 * 1024;
/// What each entry costs against the total besides its key and value, so the namespace's index
/// stays small however short the entries are: at most 16384 of them.
pub const APP_KV_ENTRY_OVERHEAD_BYTES: u64 = 256;
/// The most keys one `AppKvList` answers with.
pub const APP_KV_LIST_PAGE: usize = 100;

/// Which install's store, and of which scope. The account is the session's, never the call's.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
#[serde(deny_unknown_fields)]
pub struct AppKvNamespace {
    pub app_id: String,
    pub scope_key: String,
}

/// One operation on the namespace.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum AppKvOp {
    Get {
        key: String,
    },
    Set {
        key: String,
        #[cfg_attr(feature = "typescript", ts(type = "number[]"))]
        #[debug(with = crate::plaintext_debug_fmt)]
        value: Vec<u8>,
    },
    Delete {
        key: String,
    },
    /// Keys starting with `prefix`, in byte order, after `after` (exclusive).
    List {
        prefix: Option<String>,
        after: Option<String>,
    },
}

/// What an `AppKv` request found or did.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum AppKvResult {
    /// `Get`: the value, or none for a key never set or deleted.
    Value(
        #[cfg_attr(feature = "typescript", ts(type = "number[] | null"))]
        #[debug(with = opt_plaintext_debug_fmt)]
        Option<Vec<u8>>,
    ),
    /// `Set` or `Delete` done.
    Done,
    /// `List`: up to `APP_KV_LIST_PAGE` keys, and the `after` for the next page if there is one.
    Keys {
        keys: Vec<String>,
        next: Option<String>,
    },
}

fn opt_plaintext_debug_fmt(
    value: &Option<Vec<u8>>,
    f: &mut std::fmt::Formatter,
) -> std::fmt::Result {
    match value {
        Some(bytes) => crate::plaintext_debug_fmt(bytes, f),
        None => f.write_str("None"),
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct AppKvResponse {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub request_id: Option<Uuid>,
    pub result: AppKvResult,
}

/// Why an `AppKv` request was refused. Closed, so storage internals never reach an app.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum AppKvRefusal {
    /// The app id or scope key is not one, or the session has no account name.
    Namespace,
    /// The key is empty or longer than `APP_KV_MAX_KEY_BYTES`.
    Key,
    /// The value is longer than `APP_KV_MAX_VALUE_BYTES`.
    Value,
    /// The write would take the namespace past `APP_KV_MAX_TOTAL_BYTES`.
    Quota,
    /// The account's store could not be read or written.
    Storage,
    /// The session is not this connection's, or is not signed in.
    Session,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct AppKvFailure {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub request_id: Option<Uuid>,
    pub refusal: AppKvRefusal,
}
