//! Reading a peer's bytes back into what ILM needs.

use super::advertisement;
use super::{InternalMessage, WireWrapper};
use intersession_layer_messaging::compression::{self, Codec, CompressionError};
use intersession_layer_messaging::CapabilityEvidence;

/// One decoded frame from a peer.
#[derive(Debug)]
pub enum Decoded {
    Data {
        source: u64,
        destination: u64,
        message_id: u64,
        /// Uncompressed: exactly the bytes the sending application handed over.
        contents: Vec<u8>,
        piggybacked_ack: Option<u64>,
    },
    Control {
        signal: InternalMessage,
        evidence: CapabilityEvidence,
    },
}

#[derive(Debug, PartialEq)]
pub enum DecodeError {
    /// Not an ILM frame at all. Raw traffic -- Yjs updates, the plain
    /// messaging service -- takes this route by design and is forwarded as is.
    NotAFrame(String),
    /// An ILM frame whose extension cannot be honoured: an unknown codec, a
    /// dictionary this build does not have, contents that do not decompress
    /// or would expand past the ceiling. It is dropped, loudly, and never
    /// forwarded: these bytes are not a message anyone should see.
    Unreadable(String),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::NotAFrame(reason) => write!(f, "not an ILM frame: {reason}"),
            DecodeError::Unreadable(reason) => write!(f, "unreadable ILM frame: {reason}"),
        }
    }
}

pub fn decode_notification(bytes: &[u8]) -> Result<Decoded, DecodeError> {
    // `deserialize_from` over a slice advances it, so what is left afterwards
    // is exactly the bytes after the frame: where an advertisement lives.
    let mut rest: &[u8] = bytes;
    let wrapper: WireWrapper = bincode2::deserialize_from(&mut rest)
        .map_err(|err| DecodeError::NotAFrame(err.to_string()))?;
    match wrapper {
        WireWrapper::Message {
            source,
            destination,
            message_id,
            contents,
        } => Ok(Decoded::Data {
            source,
            destination,
            message_id,
            contents,
            piggybacked_ack: None,
        }),
        WireWrapper::ISMAux { signal } => {
            let evidence = match &*signal {
                InternalMessage::Ack { .. } | InternalMessage::Poll { .. } => {
                    CapabilityEvidence::Advertised(advertisement::read(rest))
                }
                // A loopback payload, not a peer's control frame: it says
                // nothing about what the peer accepts.
                InternalMessage::Message(_) => CapabilityEvidence::Silent,
            };
            Ok(Decoded::Control {
                signal: *signal,
                evidence,
            })
        }
        WireWrapper::MessageV2 {
            source,
            destination,
            message_id,
            codec,
            dict_id,
            piggybacked_ack,
            contents,
        } => {
            if dict_id != 0 {
                return Err(DecodeError::Unreadable(format!(
                    "frame {message_id} from {source} uses dictionary {dict_id}, which this build does not have"
                )));
            }
            let codec = Codec::from_wire(codec).map_err(unreadable(message_id, source))?;
            let contents =
                compression::decode(codec, contents).map_err(unreadable(message_id, source))?;
            Ok(Decoded::Data {
                source,
                destination,
                message_id,
                contents,
                piggybacked_ack,
            })
        }
    }
}

fn unreadable(message_id: u64, source: u64) -> impl FnOnce(CompressionError) -> DecodeError {
    move |err| DecodeError::Unreadable(format!("frame {message_id} from {source}: {err}"))
}
