//! What windows and the agent say to each other about conversations: account
//! preferences, change events, and the answers to conversation requests.

use crate::conversation::{ConversationMessage, ConversationMetadata, ConversationPage};
use crate::plaintext_debug_fmt;
use custom_debug::Debug;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg(feature = "typescript")]
use ts_rs::TS;

/// How a notification about a message reads: who sent it, or also what it says.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum NotificationPreview {
    SenderOnly,
    Text,
}

/// How long a conversation keeps its messages.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum Retention {
    Forever,
    Days(u32),
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct PeerRetention {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub peer_cid: u64,
    pub retention: Retention,
}

/// The account's settings the agent acts on with no window open. Every field is
/// stated by the UI that pushes them; the agent supplies no defaults of its own
/// beyond the UI's documented ones (`AccountPreferences::UI_DEFAULTS`).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct AccountPreferences {
    pub send_read_receipts: bool,
    pub accept_requests_from_strangers: bool,
    pub notify_on_screenshot: bool,
    pub notification_preview: NotificationPreview,
    pub retention: Vec<PeerRetention>,
}

impl AccountPreferences {
    /// The web UI's own defaults (lib/privacy-settings.ts, chat-advanced-settings.ts),
    /// which is what an account that never pushed its settings has been running with.
    pub const UI_DEFAULTS: AccountPreferences = AccountPreferences {
        send_read_receipts: true,
        accept_requests_from_strangers: true,
        notify_on_screenshot: false,
        // Privacy first: a lock screen shows who wrote, not what, until the
        // user turns previews on for the account.
        notification_preview: NotificationPreview::SenderOnly,
        retention: Vec::new(),
    };

    pub fn retention_for(&self, peer_cid: u64) -> Retention {
        self.retention
            .iter()
            .find(|r| r.peer_cid == peer_cid)
            .map(|r| r.retention)
            .unwrap_or(Retention::Forever)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum ConversationEventKind {
    /// A message was added (inbound, sent, or a record a window filed).
    Appended,
    /// A stored message changed: status, edit, reaction, transfer progress.
    Updated,
    /// A message was deleted.
    Removed,
    /// The conversation was cleared.
    Cleared,
    /// Retention removed messages older than its cutoff.
    Expired,
    /// The conversation's metadata changed (unread count, peer name).
    MetadataChanged,
}

/// Something changed in one of the account's conversations. Sent to every
/// window attached to the session; a window that sees a gap in `seq` re-reads
/// the conversation with `ConversationPage`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct ConversationEvent {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub peer_cid: u64,
    /// Per account, increasing by one per event.
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub seq: u64,
    pub kind: ConversationEventKind,
    pub message: Option<ConversationMessage>,
    pub message_id: Option<String>,
    pub metadata: Option<ConversationMetadata>,
    /// For a native notification (phase 6): whose account, and from whom.
    pub account_username: String,
    pub peer_username: Option<String>,
    /// The first 100 characters of an appended message's text.
    #[debug(with = plaintext_debug_fmt)]
    pub preview: String,
    pub request_id: Option<Uuid>,
}

/// A request about a conversation succeeded; carries the message it touched.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct ConversationUpdated {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub peer_cid: u64,
    pub message: Option<ConversationMessage>,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct ConversationFailure {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct ConversationListResponse {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub conversations: Vec<ConversationMetadata>,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct ConversationPageResponse {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub peer_cid: u64,
    pub metadata: Option<ConversationMetadata>,
    pub page: Option<ConversationPage>,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct AccountPreferencesResponse {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub preferences: AccountPreferences,
    pub request_id: Option<Uuid>,
}
