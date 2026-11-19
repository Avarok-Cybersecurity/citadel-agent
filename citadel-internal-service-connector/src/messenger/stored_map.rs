//! The on-disk format of ILM's two message queues, versioned.
//!
//! The queues were stored as bare `bincode2` of `State`, whose messages hold an
//! `InternalServicePayload`. bincode writes an enum variant as its POSITION, and
//! `InternalServiceRequest` gains variants in the middle: between agent 0.11.0
//! and 0.13.0 eleven were inserted before `Message`, moving it from index 2 to
//! 13. A queue written by 0.11.0 therefore failed to decode in 0.13.0
//! ("expected variant index 0 <= i < 2"), and every new send was refused, since
//! a send has to read the queue first. A queue of ONE message was worse: index 2
//! is now `IdentityStatus`, and it decoded "successfully" as that.
//!
//! The fix is a format of our own, independent of the wire enums:
//! - an envelope (`MAGIC`, `FORMAT_V1`) so a reader knows what it holds;
//! - `StoredContents`, an enum used ONLY here, append-only, whose variants copy
//!   the fields of the two payloads ILM queues; anything else is kept
//!   self-described (variant names, not positions) as JSON.
//!
//! A blob without the envelope is the legacy format. It is accepted only if it
//! decodes with nothing left over AND every message is the payload that queue
//! holds, for this account; otherwise it is `Unreadable` and the caller
//! quarantines it (keeps the bytes under another key) instead of failing every
//! send forever. Pure: no I/O here.

use crate::messenger::backend_map::State;
use crate::messenger::WrappedMessage;
use citadel_internal_service_types::{
    InternalServicePayload, InternalServiceRequest, InternalServiceResponse, MessageNotification,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

pub(crate) const MAGIC: [u8; 4] = *b"ILMQ";
pub(crate) const FORMAT_V1: u8 = 1;

/// Which queue a blob is, which decides the one payload its legacy form may hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueKind {
    Outbound,
    Inbound,
}

/// What a stored blob turned out to be.
#[derive(Debug)]
pub enum Decoded {
    Current(State),
    /// The pre-envelope format, and it passed validation: rewrite it as current.
    LegacyAccepted(State),
    /// Neither format, or legacy that decoded into something this queue never
    /// holds. The bytes must be kept elsewhere, never parsed into a queue.
    Unreadable(String),
}

/// APPEND-ONLY: a variant's position is its on-disk tag. Never reorder or
/// insert; `stored_map_tests::the_tags_are_pinned` fails if one moves.
#[derive(Serialize, Deserialize)]
enum StoredContents {
    OutboundMessage {
        request_id: Uuid,
        message: Vec<u8>,
        cid: u64,
        peer_cid: Option<u64>,
        security_level: u8,
    },
    InboundNotification {
        message: Vec<u8>,
        cid: u64,
        peer_cid: u64,
        request_id: Option<Uuid>,
    },
    SelfDescribed {
        json: Vec<u8>,
    },
}

#[derive(Serialize, Deserialize)]
struct StoredMessage {
    source_id: u64,
    destination_id: u64,
    message_id: u64,
    contents: StoredContents,
}

type StoredState = HashMap<u64, HashMap<u64, StoredMessage>>;

fn to_stored(contents: &InternalServicePayload) -> Result<StoredContents, String> {
    Ok(match contents {
        InternalServicePayload::Request(InternalServiceRequest::Message {
            request_id,
            message,
            cid,
            peer_cid,
            security_level,
        }) => StoredContents::OutboundMessage {
            request_id: *request_id,
            message: message.clone(),
            cid: *cid,
            peer_cid: *peer_cid,
            security_level: security_level.value(),
        },
        InternalServicePayload::Response(InternalServiceResponse::MessageNotification(n)) => {
            StoredContents::InboundNotification {
                message: n.message.clone(),
                cid: n.cid,
                peer_cid: n.peer_cid,
                request_id: n.request_id,
            }
        }
        other => StoredContents::SelfDescribed {
            json: serde_json::to_vec(other).map_err(|e| format!("self-described payload: {e}"))?,
        },
    })
}

fn from_stored(contents: StoredContents) -> Result<InternalServicePayload, String> {
    Ok(match contents {
        StoredContents::OutboundMessage {
            request_id,
            message,
            cid,
            peer_cid,
            security_level,
        } => InternalServicePayload::Request(InternalServiceRequest::Message {
            request_id,
            message,
            cid,
            peer_cid,
            security_level: security_level.into(),
        }),
        StoredContents::InboundNotification {
            message,
            cid,
            peer_cid,
            request_id,
        } => InternalServicePayload::Response(InternalServiceResponse::MessageNotification(
            MessageNotification {
                message,
                cid,
                peer_cid,
                request_id,
            },
        )),
        StoredContents::SelfDescribed { json } => {
            serde_json::from_slice(&json).map_err(|e| format!("self-described payload: {e}"))?
        }
    })
}

/// The current format: envelope, then `StoredState`.
pub fn encode(state: &State) -> Result<Vec<u8>, String> {
    let mut stored = StoredState::new();
    for (peer, messages) in state {
        let per_peer = stored.entry(*peer).or_default();
        for (id, m) in messages {
            let contents = to_stored(&m.contents)?;
            per_peer.insert(
                *id,
                StoredMessage {
                    source_id: m.source_id,
                    destination_id: m.destination_id,
                    message_id: m.message_id,
                    contents,
                },
            );
        }
    }
    let body = bincode2::serialize(&stored).map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(MAGIC.len() + 1 + body.len());
    out.extend_from_slice(&MAGIC);
    out.push(FORMAT_V1);
    out.extend_from_slice(&body);
    Ok(out)
}

pub fn decode(bytes: &[u8], kind: QueueKind, owner_cid: u64) -> Decoded {
    match bytes.strip_prefix(&MAGIC[..]) {
        Some([FORMAT_V1, body @ ..]) => match decode_current(body) {
            Ok(state) => Decoded::Current(state),
            Err(why) => Decoded::Unreadable(format!("format {FORMAT_V1}: {why}")),
        },
        Some(rest) => Decoded::Unreadable(format!(
            "unknown format version {:?} (written by a newer agent?)",
            rest.first()
        )),
        None => match decode_legacy(bytes, kind, owner_cid) {
            Ok(state) => Decoded::LegacyAccepted(state),
            Err(why) => Decoded::Unreadable(format!("legacy format: {why}")),
        },
    }
}

fn exactly<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    let mut rest = bytes;
    let value = bincode2::deserialize_from(&mut rest).map_err(|e| e.to_string())?;
    if rest.is_empty() {
        Ok(value)
    } else {
        Err(format!("{} bytes left over", rest.len()))
    }
}

fn decode_current(body: &[u8]) -> Result<State, String> {
    let stored: StoredState = exactly(body)?;
    let mut state = State::new();
    for (peer, messages) in stored {
        let per_peer = state.entry(peer).or_default();
        for (id, m) in messages {
            let contents = from_stored(m.contents)?;
            per_peer.insert(
                id,
                WrappedMessage {
                    source_id: m.source_id,
                    destination_id: m.destination_id,
                    message_id: m.message_id,
                    contents,
                },
            );
        }
    }
    Ok(state)
}

fn decode_legacy(bytes: &[u8], kind: QueueKind, owner_cid: u64) -> Result<State, String> {
    let state: State = exactly(bytes)?;
    for m in state.values().flat_map(HashMap::values) {
        let belongs = match (kind, &m.contents) {
            (
                QueueKind::Outbound,
                InternalServicePayload::Request(InternalServiceRequest::Message { cid, .. }),
            ) => *cid == owner_cid,
            (
                QueueKind::Inbound,
                InternalServicePayload::Response(InternalServiceResponse::MessageNotification(n)),
            ) => n.cid == owner_cid,
            _ => false,
        };
        if !belongs {
            return Err(format!(
                "message {} is not a {kind:?} message of session {owner_cid}",
                m.message_id
            ));
        }
    }
    Ok(state)
}

#[cfg(test)]
#[path = "stored_map_tests.rs"]
mod stored_map_tests;
