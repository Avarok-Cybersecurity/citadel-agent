//! The bytes a conversation page or its metadata occupies in LocalDB.
//!
//! UTF-8 JSON in the canonical types' field names, with three differences the
//! web UI has always written and so every stored history has: CIDs are decimal
//! strings, a message's reactions are the cbor-x bytes of the list (as a JSON
//! array of numbers), and an absent optional field is omitted. This module is
//! the only place the difference lives; everything else uses the canonical
//! types (citadel-internal-service-types `conversation.rs`).

use super::reactions;
use citadel_internal_service_types::{ConversationMetadata, ConversationPage};
use serde_json::{Map, Value};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct StoredError(pub String);

const CID_FIELDS_MESSAGE: [&str; 2] = ["senderCid", "recipientCid"];

pub(crate) fn encode_page(page: &ConversationPage) -> Result<Vec<u8>, StoredError> {
    let mut value = to_value(page)?;
    let object = as_object(&mut value)?;
    cid_to_string(object, "peerCid")?;
    if let Some(Value::Array(messages)) = object.get_mut("messages") {
        for message in messages {
            let message = as_object(message)?;
            for field in CID_FIELDS_MESSAGE {
                cid_to_string(message, field)?;
            }
            stored_reactions(message)?;
        }
    }
    js_numbers(&mut value);
    serde_json::to_vec(&value).map_err(err)
}

pub(crate) fn decode_page(bytes: &[u8]) -> Result<ConversationPage, StoredError> {
    let mut value: Value = serde_json::from_slice(bytes).map_err(err)?;
    let object = as_object(&mut value)?;
    cid_from_string(object, "peerCid")?;
    if let Some(Value::Array(messages)) = object.get_mut("messages") {
        for message in messages {
            let message = as_object(message)?;
            for field in CID_FIELDS_MESSAGE {
                cid_from_string(message, field)?;
            }
            canonical_reactions(message);
        }
    }
    serde_json::from_value(value).map_err(err)
}

pub(crate) fn encode_metadata(metadata: &ConversationMetadata) -> Result<Vec<u8>, StoredError> {
    let mut value = to_value(metadata)?;
    let object = as_object(&mut value)?;
    cid_to_string(object, "peerCid")?;
    cid_to_string(object, "ownerCid")?;
    js_numbers(&mut value);
    serde_json::to_vec(&value).map_err(err)
}

pub(crate) fn decode_metadata(bytes: &[u8]) -> Result<ConversationMetadata, StoredError> {
    let mut value: Value = serde_json::from_slice(bytes).map_err(err)?;
    let object = as_object(&mut value)?;
    cid_from_string(object, "peerCid")?;
    cid_from_string(object, "ownerCid")?;
    serde_json::from_value(value).map_err(err)
}

/// Values as `JSON.stringify` writes them: an integral number has no `.0`,
/// and an absent field (`undefined` in JS, `null` here) is not written at all.
fn js_numbers(value: &mut Value) {
    if let Value::Object(map) = value {
        map.retain(|_, v| !v.is_null());
    }
    match value {
        Value::Number(n) => {
            if let Some(f) = n
                .as_f64()
                .filter(|f| f.fract() == 0.0 && f.abs() < 9.007_199_254_740_992e15)
            {
                if n.is_f64() {
                    *value = Value::from(f as i64);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(js_numbers),
        Value::Object(map) => map.values_mut().for_each(js_numbers),
        _ => {}
    }
}

fn err(e: impl std::fmt::Display) -> StoredError {
    StoredError(e.to_string())
}

fn to_value(value: &impl serde::Serialize) -> Result<Value, StoredError> {
    serde_json::to_value(value).map_err(err)
}

fn as_object(value: &mut Value) -> Result<&mut Map<String, Value>, StoredError> {
    value
        .as_object_mut()
        .ok_or_else(|| StoredError("not a JSON object".to_string()))
}

/// A u64 does not survive JSON in JS (it is a double there), so CIDs travel as
/// decimal strings.
fn cid_to_string(object: &mut Map<String, Value>, field: &str) -> Result<(), StoredError> {
    if let Some(value) = object.get_mut(field).filter(|v| !v.is_null()) {
        let cid = value
            .as_u64()
            .ok_or_else(|| StoredError(format!("{field} is not a CID")))?;
        *value = Value::String(cid.to_string());
    }
    Ok(())
}

/// Older writers may have stored a CID as a string or a number; both read.
fn cid_from_string(object: &mut Map<String, Value>, field: &str) -> Result<(), StoredError> {
    match object.get(field) {
        Some(Value::String(text)) => {
            let cid: u64 = text
                .parse()
                .map_err(|_| StoredError(format!("{field} is not a CID: {text:?}")))?;
            object.insert(field.to_string(), Value::from(cid));
        }
        Some(Value::Null) => {
            object.remove(field);
        }
        _ => {}
    }
    Ok(())
}

fn stored_reactions(message: &mut Map<String, Value>) -> Result<(), StoredError> {
    let Some(value) = message.remove("reactions").filter(|v| !v.is_null()) else {
        return Ok(());
    };
    let list: Vec<citadel_internal_service_types::Reaction> =
        serde_json::from_value(value).map_err(err)?;
    if let Some(bytes) = reactions::encode(&list) {
        message.insert("reactions".to_string(), Value::from(bytes));
    }
    Ok(())
}

/// The stored byte array back to a list, dropped (not failed) when unreadable,
/// as the UI's `decodeStoredReactions` does.
fn canonical_reactions(message: &mut Map<String, Value>) {
    let Some(value) = message.remove("reactions") else {
        return;
    };
    let bytes: Option<Vec<u8>> = serde_json::from_value(value).ok();
    if let Some(list) = bytes.as_deref().and_then(reactions::decode) {
        if let Ok(value) = serde_json::to_value(list) {
            message.insert("reactions".to_string(), value);
        }
    }
}

#[cfg(test)]
#[path = "stored_tests.rs"]
mod tests;
