//! The P2P commands the agent reads and writes, in the web UI's encoding.
//!
//! The UI's types/p2p-commands.ts and messaging-layer.ts define the shapes;
//! cbor-x defines the bytes (cbor.rs). Every builder here produces exactly
//! the bytes the UI's constructor would, pinned against fixtures the UI's
//! encoder wrote (tests/fixtures/p2p_commands).

use super::cbor::Value;
use citadel_internal_service_types::{Attachment, MessageType};

/// The envelope fields of a `MessagingLayerCommand`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Envelope {
    pub sender_cid: u64,
    pub recipient_cid: u64,
    pub message_id: String,
    pub index: f64,
    pub reply_to: Option<String>,
    pub mentions: Option<Vec<String>>,
    pub attachments: Option<Vec<Attachment>>,
    pub message_type: MessageType,
    pub document_id: Option<String>,
    pub document_title: Option<String>,
}

/// What an arriving command means to the conversation store.
#[derive(Debug, PartialEq)]
pub(crate) enum Inbound {
    Message {
        envelope: Envelope,
        contents: String,
        timestamp: f64,
    },
    Edit {
        target: String,
        contents: String,
        edited_at: f64,
    },
    Delete {
        target: String,
    },
    Reaction {
        target: String,
        emoji: String,
        active: bool,
        at: f64,
    },
    Screenshot {
        envelope: Envelope,
        taken_at: Option<f64>,
    },
    Ack {
        kind: AckKind,
        message_id: String,
    },
    /// "Are you there?": the agent answers it itself.
    CheckState,
    /// Momentary state (typing, presence, a readiness answer, call signalling):
    /// for a window if one is open, and of no use later.
    Ephemeral,
    /// Something only a window can act on (file transfer, RE-VFS, unknown).
    ForAWindow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AckKind {
    Delivered,
    Read,
    Failed,
}

impl AckKind {
    pub(super) fn name(self) -> &'static str {
        match self {
            AckKind::Delivered => "delivered",
            AckKind::Read => "read",
            AckKind::Failed => "failed",
        }
    }
}

fn string(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(str::to_string)
}

pub(super) fn opt_text(v: &Option<String>) -> Value {
    v.as_ref()
        .map_or(Value::Undefined, |t| Value::text(t.clone()))
}

fn message_type(name: Option<&str>) -> MessageType {
    serde_json::from_value(serde_json::Value::String(
        name.unwrap_or("text").to_string(),
    ))
    .unwrap_or_default()
}

pub(super) fn message_type_name(t: MessageType) -> String {
    match serde_json::to_value(t) {
        Ok(serde_json::Value::String(name)) => name,
        _ => "text".to_string(),
    }
}

fn envelope(payload: &Value) -> Option<Envelope> {
    Some(Envelope {
        sender_cid: payload.get("sender_cid")?.as_bigint()?,
        recipient_cid: payload.get("recipient_cid")?.as_bigint()?,
        message_id: string(payload.get("message_id"))?,
        index: payload
            .get("index")
            .and_then(Value::as_number)
            .unwrap_or(0.0),
        reply_to: string(payload.get("reply_to")),
        mentions: match payload.get("mentions") {
            Some(Value::Array(items)) => Some(
                items
                    .iter()
                    .filter_map(|i| i.as_str().map(str::to_string))
                    .collect(),
            ),
            _ => None,
        },
        attachments: match payload.get("attachments") {
            Some(Value::Array(items)) => Some(items.iter().filter_map(attachment).collect()),
            _ => None,
        },
        message_type: message_type(payload.get("message_type").and_then(Value::as_str)),
        document_id: string(payload.get("document_id")),
        document_title: string(payload.get("document_title")),
    })
}

fn attachment(v: &Value) -> Option<Attachment> {
    Some(Attachment {
        file_id: string(v.get("file_id"))?,
        file_name: string(v.get("file_name"))?,
        file_size: v.get("file_size")?.as_number()?,
        file_type: string(v.get("file_type"))?,
        thumbnail: string(v.get("thumbnail")),
    })
}

/// Read one arriving P2P payload. `None` is bytes that are not a P2P command
/// at all; they belong to a window.
pub(crate) fn read(bytes: &[u8]) -> Option<Inbound> {
    let command = Value::decode(bytes).ok()?;
    let payload = command.get("payload")?;
    Some(match command.get("type")?.as_str()? {
        "MessageAck" => Inbound::Ack {
            kind: match payload.get("ack_type")?.as_str()? {
                "delivered" => AckKind::Delivered,
                "read" => AckKind::Read,
                "failed" => AckKind::Failed,
                _ => return Some(Inbound::ForAWindow),
            },
            message_id: string(payload.get("message_id"))?,
        },
        "CallSignal" => Inbound::Ephemeral,
        "MessagingLayerCommand" => {
            let layer = payload.get("layer")?;
            let num = |key: &str| layer.get(key).and_then(Value::as_number);
            match layer.get("type")?.as_str()? {
                "Message" => Inbound::Message {
                    envelope: envelope(payload)?,
                    contents: string(layer.get("contents"))?,
                    timestamp: num("timestamp")?,
                },
                "MessageEdit" => Inbound::Edit {
                    target: string(layer.get("message_id"))?,
                    contents: string(layer.get("contents"))?,
                    edited_at: num("edited_at")?,
                },
                "MessageDelete" => Inbound::Delete {
                    target: string(layer.get("message_id"))?,
                },
                "MessageReaction" => Inbound::Reaction {
                    target: string(layer.get("message_id"))?,
                    emoji: string(layer.get("emoji"))?,
                    active: layer.get("active")?.as_bool()?,
                    at: num("reacted_at")?,
                },
                "ScreenshotNotice" => Inbound::Screenshot {
                    envelope: envelope(payload)?,
                    taken_at: num("taken_at"),
                },
                "CheckState" => Inbound::CheckState,
                "Typing" | "Away" | "Online" | "Offline" | "CustomState" | "CheckStateResponse" => {
                    Inbound::Ephemeral
                }
                _ => Inbound::ForAWindow,
            }
        }
        _ => Inbound::ForAWindow,
    })
}

#[path = "command_build.rs"]
mod build;
pub(crate) use build::*;

#[cfg(test)]
#[path = "command_tests.rs"]
mod tests;
