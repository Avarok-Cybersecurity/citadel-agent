//! An account's 1:1 conversations, as the agent keeps them.
//!
//! The agent is the only writer of the conversation store (citadel-workspace
//! `docs/plans/multi-window-sessions.md`, mw4), so these types ARE the format:
//! the TypeScript the UI uses is generated from them. Field names keep the
//! spelling the web UI's stored pages have always used, so history written by
//! older builds reads back unchanged.
//!
//! No field is skipped when absent: the agent's TCP clients speak bincode,
//! which has no notion of a missing field, so every field is always written.
//! The stored JSON omits absent fields itself (kernel/conversations/stored.rs).

use crate::plaintext_debug_fmt;
use custom_debug::Debug;
use serde::{Deserialize, Serialize};

#[cfg(feature = "typescript")]
use ts_rs::TS;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
#[serde(rename_all = "lowercase")]
pub enum MessageStatus {
    Pending,
    Sent,
    Delivered,
    Read,
    Failed,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    #[default]
    Text,
    Markdown,
    LiveDocument,
    FileTransfer,
    SystemNotice,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
#[serde(rename_all = "lowercase")]
pub enum TransferMode {
    Async,
    P2p,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
#[serde(rename_all = "lowercase")]
pub enum TransferState {
    Pending,
    Uploading,
    Staged,
    Transferring,
    Complete,
    Declined,
    Cancelled,
    Expired,
    Error,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct Attachment {
    pub file_id: String,
    pub file_name: String,
    pub file_size: f64,
    pub file_type: String,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub thumbnail: Option<String>,
}

/// One reactor's one emoji on a message; retractions stay as `active: false`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct Reaction {
    pub emoji: String,
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub reactor_cid: u64,
    pub at: f64,
    pub active: bool,
}

/// One message in a conversation.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct ConversationMessage {
    pub id: String,
    #[debug(with = plaintext_debug_fmt)]
    pub content: String,
    #[serde(rename = "senderCid")]
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub sender_cid: u64,
    #[serde(rename = "recipientCid")]
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub recipient_cid: u64,
    pub timestamp: f64,
    pub index: f64,
    pub status: MessageStatus,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub error: Option<String>,
    #[serde(rename = "replyTo", default)]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub reply_to: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub edited_at: Option<f64>,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub reactions: Option<Vec<Reaction>>,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub mentions: Option<Vec<String>>,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub attachments: Option<Vec<Attachment>>,
    #[serde(default)]
    pub message_type: MessageType,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub document_id: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub document_title: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub transfer_id: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub file_name: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub file_size: Option<f64>,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub file_type: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub file_thumbnail: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub transfer_mode: Option<TransferMode>,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub transfer_state: Option<TransferState>,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub transfer_progress: Option<f64>,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub virtual_path: Option<String>,
}

/// What a window may change on a message it did not author the content of:
/// delivery state and file-transfer progress. Absent fields are left alone.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct MessagePatch {
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub status: Option<MessageStatus>,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub error: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub transfer_state: Option<TransferState>,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub transfer_progress: Option<f64>,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub virtual_path: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub file_thumbnail: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ConversationMetadata {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub peer_cid: u64,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional, type = "bigint"))]
    pub owner_cid: Option<u64>,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub peer_username: Option<String>,
    pub total_message_count: f64,
    pub oldest_message_timestamp: f64,
    pub newest_message_timestamp: f64,
    pub latest_page: u32,
    pub messages_per_page: u32,
    pub unread_count: f64,
    pub last_message_index: f64,
    pub last_updated: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct PageTimestamps {
    pub min_timestamp: f64,
    pub max_timestamp: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ConversationPage {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub peer_cid: u64,
    pub page_number: u32,
    pub messages: Vec<ConversationMessage>,
    pub page_timestamps: PageTimestamps,
}
