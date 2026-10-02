//! Building the P2P commands the agent sends, as the UI's constructors build them.

use super::{message_type_name, opt_text, AckKind, Envelope};
use crate::kernel::conversations::cbor::Value;

/// `createMessagingLayerCommand(layer, ...)`.
pub(crate) fn messaging_layer(layer: Value, e: &Envelope) -> Vec<u8> {
    let attachments = e.attachments.as_ref().map_or(Value::Undefined, |list| {
        Value::Array(
            list.iter()
                .map(|a| {
                    Value::object(vec![
                        ("file_id", Value::text(a.file_id.clone())),
                        ("file_name", Value::text(a.file_name.clone())),
                        ("file_size", Value::number(a.file_size)),
                        ("file_type", Value::text(a.file_type.clone())),
                        ("thumbnail", opt_text(&a.thumbnail)),
                    ])
                })
                .collect(),
        )
    });
    let mentions = e.mentions.as_ref().map_or(Value::Undefined, |list| {
        Value::Array(list.iter().map(|m| Value::text(m.clone())).collect())
    });
    Value::object(vec![
        ("type", Value::text("MessagingLayerCommand")),
        (
            "payload",
            Value::object(vec![
                ("layer", layer),
                ("sender_cid", Value::bigint(e.sender_cid)),
                ("recipient_cid", Value::bigint(e.recipient_cid)),
                ("message_id", Value::text(e.message_id.clone())),
                ("index", Value::number(e.index)),
                ("reply_to", opt_text(&e.reply_to)),
                ("mentions", mentions),
                ("attachments", attachments),
                (
                    "message_type",
                    Value::text(message_type_name(e.message_type)),
                ),
                ("document_id", opt_text(&e.document_id)),
                ("document_title", opt_text(&e.document_title)),
            ]),
        ),
    ])
    .encode()
}

pub(crate) fn message_layer(contents: &str, timestamp: f64) -> Value {
    Value::object(vec![
        ("type", Value::text("Message")),
        ("contents", Value::text(contents)),
        ("timestamp", Value::number(timestamp)),
    ])
}

pub(crate) fn edit_layer(target: &str, contents: &str, edited_at: f64) -> Value {
    Value::object(vec![
        ("type", Value::text("MessageEdit")),
        ("message_id", Value::text(target)),
        ("contents", Value::text(contents)),
        ("edited_at", Value::number(edited_at)),
    ])
}

pub(crate) fn delete_layer(target: &str, deleted_at: f64) -> Value {
    Value::object(vec![
        ("type", Value::text("MessageDelete")),
        ("message_id", Value::text(target)),
        ("deleted_at", Value::number(deleted_at)),
    ])
}

pub(crate) fn reaction_layer(target: &str, emoji: &str, active: bool, at: f64) -> Value {
    Value::object(vec![
        ("type", Value::text("MessageReaction")),
        ("message_id", Value::text(target)),
        ("emoji", Value::text(emoji)),
        ("active", Value::Bool(active)),
        ("reacted_at", Value::number(at)),
    ])
}

pub(crate) fn check_state_response() -> Value {
    Value::object(vec![
        ("type", Value::text("CheckStateResponse")),
        ("ready", Value::Bool(true)),
    ])
}

/// `createMessageAckCommand(messageId, ackType)` (no error).
pub(crate) fn ack(kind: AckKind, message_id: &str, timestamp: f64) -> Vec<u8> {
    Value::object(vec![
        ("type", Value::text("MessageAck")),
        (
            "payload",
            Value::object(vec![
                ("ack_type", Value::text(kind.name())),
                ("message_id", Value::text(message_id)),
                ("timestamp", Value::number(timestamp)),
                ("error", Value::Undefined),
            ]),
        ),
    ])
    .encode()
}
