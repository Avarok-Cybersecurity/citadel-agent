//! App notices: what a Citadel app (the Calendar) asks the agent to show natively, now or at a
//! time. The agent fires scheduled ones itself, so a reminder reaches the menu-bar app with no
//! window open, and tells the session's windows what it fired so the bell lists it.
//!
//! A notice names the node to open, never a URL: the click opens `open=node:<node_id>` on the
//! workspace's own origin, the way every other native notice does (`NoticeTarget`).

use crate::{plaintext_debug_fmt, secret_debug_fmt};
use custom_debug::Debug;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg(feature = "typescript")]
use ts_rs::TS;

/// One notice from an app. `fire_at_ms` absent is "show it now"; present is a reminder the agent
/// keeps and fires at that instant (Unix ms, UTC).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct AppNotice {
    /// Stable across syncs and shared with the browser path: `reminder:<event_id>:<at_ms>`. A
    /// notice with an id already fired is never fired again.
    pub id: String,
    pub app_id: String,
    /// The node the app's instance is mounted on: what a click opens.
    pub node_id: String,
    #[debug(with = plaintext_debug_fmt)]
    pub title: String,
    #[debug(with = plaintext_debug_fmt)]
    pub body: String,
    /// Present on a notice that carries someone else's words (an invitation, a booking, a poll):
    /// all it says while the account shows only who sent things, e.g. "Calendar: invitation from
    /// Alice". Absent on a reminder of the member's own event, whose title always shows.
    #[debug(with = secret_debug_fmt)]
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(type = "string | undefined"))]
    pub sender_only: Option<String>,
    #[cfg_attr(feature = "typescript", ts(type = "bigint | undefined"))]
    pub fire_at_ms: Option<u64>,
    /// When what the reminder is for begins.
    #[cfg_attr(feature = "typescript", ts(type = "bigint | undefined"))]
    pub start_ms: Option<u64>,
}

/// Why an app-notice request was refused.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum AppNoticeRefusal {
    /// The connection is not attached to the session it named.
    NotAttached,
    /// A notice's id, app, node, title or body is not one the agent accepts, or a reminder has
    /// no `fire_at_ms` or names another app than the sync.
    BadTarget,
    /// The session would hold more reminders than `cap`; nothing was stored.
    CapExceeded { cap: u32 },
    /// Nothing on the agent's side shows native notices right now: the page shows it itself.
    Unheard,
    /// The account's store could not be read or written.
    Store(String),
}

/// An `AppNoticePost` was shown, or an `AppRemindersSync` stored.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct AppNoticeAccepted {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct AppNoticeFailure {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub reason: AppNoticeRefusal,
    pub request_id: Option<Uuid>,
}

/// The agent fired a reminder: every window of the session is told, and the one that keeps the
/// bell lists it. `shown_natively` says the menu-bar app showed it, so the page raises no OS
/// notification of its own; otherwise the page's own path does.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct AppNoticeNotification {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub notice: AppNotice,
    /// Fired after its time: the computer slept, or nothing could show it then.
    pub late: bool,
    pub shown_natively: bool,
    pub request_id: Option<Uuid>,
}
