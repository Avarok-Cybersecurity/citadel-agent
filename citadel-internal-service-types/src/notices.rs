//! Native notifications (multi-window mw6): what the agent tells the menu-bar
//! app to show, and the account rows it lists.
//!
//! The notice plane is not a session's: one subscriber hears every signed-in
//! account on this computer, so it is open only to the native app that started
//! the agent, which proves it with the launch token it put in the agent's
//! environment (never a command-line argument, never logged).

use crate::plaintext_debug_fmt;
use custom_debug::Debug;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg(feature = "typescript")]
use ts_rs::TS;

/// What a notice is about.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum NoticeKind {
    Message,
    PeerRequest,
    GroupInvite,
    FileOffer,
    IncomingCall,
}

/// Where a click on a notice opens. The client builds the URL from its own
/// origin; this names the account, its workspace server and the target only.
/// It never carries message content: a link is shareable and lands in history.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct NoticeTarget {
    pub account: String,
    /// The workspace server as the user typed it; absent from older sessions.
    pub server_host: Option<String>,
    /// `conversation:<peer>`, `call:<peer>` or `requests`.
    pub open: String,
}

/// One OS notification to raise.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct NativeNotice {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub kind: NoticeKind,
    pub title: String,
    /// The message text only when the account shows previews; otherwise a
    /// description of what happened.
    #[debug(with = plaintext_debug_fmt)]
    pub body: String,
    pub target: NoticeTarget,
    pub request_id: Option<Uuid>,
}

/// A signed-in account, as the menu-bar app lists it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct AccountRow {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub username: String,
    pub server_host: Option<String>,
    /// Unread messages across the account's conversations.
    pub unread: u32,
    pub muted: bool,
}

/// The account rows, on subscribing and whenever one changes.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct NoticeRows {
    /// 0: the rows are every account's, not one session's.
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub rows: Vec<AccountRow>,
    pub request_id: Option<Uuid>,
}

/// A notice-plane request was refused: no token configured, or the wrong one.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct NoticeFailure {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub message: String,
    pub request_id: Option<Uuid>,
}
