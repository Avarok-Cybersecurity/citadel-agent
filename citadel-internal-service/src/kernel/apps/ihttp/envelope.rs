//! The `citadel-ihttp/1` P2P envelope: `{ type: "citadel-ihttp/1", payload: <frame bytes> }`.
//!
//! The same shape as every P2P command the web UI sends (`{ type, payload }`, cbor-x), so a
//! receiver that predates internal HTTP decodes it as a command of a type it does not know:
//! the UI's `handleP2PCommand` ignores it, and an older agent's conversation store hands it
//! to a window as something it does not store. Encoded as cbor-x encodes that object
//! (conversations/cbor.rs), so the agent's bytes and a page's agree.

use crate::kernel::conversations::cbor::Value;
use citadel_ihttp_types::{decode, encode, FrameError, IhttpFrame, ENVELOPE_KIND, MAX_FRAME_BYTES};

/// The envelope's own bytes around a frame: its map header, two keys, the kind, and the
/// payload's byte-string header.
const ENVELOPE_OVERHEAD_BYTES: usize = 64;

/// What arrived, as far as internal HTTP is concerned.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Inbound {
    /// Not an internal-HTTP envelope: chat, Yjs, file transfer, anything else.
    Other,
    /// An envelope whose frame failed to decode or validate. Dropped and counted.
    Malformed(String),
    /// A valid frame, re-encoded canonically.
    Frame(Vec<u8>),
}

/// The envelope around one validated frame.
pub(crate) fn wrap(frame: &IhttpFrame) -> Result<Vec<u8>, FrameError> {
    let bytes = encode(frame)?;
    let envelope = Value::object(vec![
        ("type", Value::text(ENVELOPE_KIND)),
        ("payload", Value::Bytes(bytes)),
    ]);
    Ok(envelope.encode())
}

/// The prefix every envelope starts with: a two-entry map (cbor-x's 16-bit header), the key
/// `type` and the value `citadel-ihttp/1`. Checked before anything is decoded, so ordinary
/// traffic costs a byte comparison, not a parse.
fn prefix() -> Vec<u8> {
    let mut out = vec![0xb9, 0x00, 0x02];
    out.extend(Value::text("type").encode());
    out.extend(Value::text(ENVELOPE_KIND).encode());
    out
}

pub(crate) fn is_envelope(bytes: &[u8]) -> bool {
    bytes.starts_with(&prefix())
}

/// Read an arriving payload: an envelope must be exactly `{type, payload}` with a byte-string
/// payload that is one valid frame.
pub(crate) fn read(bytes: &[u8]) -> Inbound {
    if !is_envelope(bytes) {
        return Inbound::Other;
    }
    let malformed = |why: &str| Inbound::Malformed(why.to_string());
    if bytes.len() > MAX_FRAME_BYTES + ENVELOPE_OVERHEAD_BYTES {
        return malformed("larger than any frame");
    }
    let Ok(Value::Map(entries, _)) = Value::decode(bytes) else {
        return malformed("not one CBOR map");
    };
    let [(_, _), (Value::Text(key), Value::Bytes(payload))] = entries.as_slice() else {
        return malformed("not exactly {type, payload: bytes}");
    };
    if key != "payload" {
        return malformed("the second key is not payload");
    }
    match decode(payload).and_then(|frame| encode(&frame)) {
        Ok(canonical) => Inbound::Frame(canonical),
        Err(why) => Inbound::Malformed(why.to_string()),
    }
}
