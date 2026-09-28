//! The bytes an ILM frame becomes between two workspace connectors.
//!
//! Three shapes, and which one a peer receives is decided by ILM, never here:
//!
//! * `WireWrapper::Message` -- the legacy data frame. Sent whenever ILM asks
//!   for no extensions, so with every option off the wire is byte-identical
//!   to a build that predates them.
//! * `WireWrapper::ISMAux` -- the legacy Ack/Poll frame, optionally followed
//!   by an advertisement that legacy peers do not read (see `advertisement`).
//! * `WireWrapper::MessageV2` -- a data frame with a piggybacked ACK and/or
//!   compressed contents. A legacy peer cannot decode it and would hand the
//!   bytes to its UI as a plain message, which is why ILM produces it only
//!   for a peer that has advertised everything it uses.

mod advertisement;
mod decode;
#[cfg(test)]
mod tests;

pub use decode::{decode_notification, DecodeError, Decoded};

use super::{InternalMessage, WrappedMessage};
use citadel_internal_service_types::{InternalServicePayload, InternalServiceRequest};
use intersession_layer_messaging::compression::{self, Codec};
use intersession_layer_messaging::{FrameExtensions, OutboundFrame, Payload};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Serialize, Deserialize, Debug)]
pub enum WireWrapper {
    Message {
        source: u64,
        destination: u64,
        message_id: u64,
        contents: Vec<u8>,
    },
    ISMAux {
        signal: Box<InternalMessage>,
    },
    /// Appended, never inserted: bincode tags variants by index, so the two
    /// above keep the encodings every deployed peer already reads.
    MessageV2 {
        source: u64,
        destination: u64,
        message_id: u64,
        /// `intersession_layer_messaging::Codec` wire id; 0 is uncompressed.
        codec: u8,
        /// Reserved for a shared dictionary. Always 0 until one exists; a
        /// receiver refuses anything else rather than decode it wrongly.
        dict_id: u16,
        piggybacked_ack: Option<u64>,
        contents: Vec<u8>,
    },
}

#[derive(Debug)]
pub enum EncodeError {
    /// ILM handed a data frame whose contents are not an outgoing message, or
    /// extensions that do not belong on its payload. Both are defects in the
    /// caller; the frame is refused rather than guessed at.
    Unencodable(String),
    Compression(compression::CompressionError),
    Serialize(String),
}

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EncodeError::Unencodable(reason) => write!(f, "cannot encode frame: {reason}"),
            EncodeError::Compression(err) => write!(f, "cannot compress frame: {err}"),
            EncodeError::Serialize(reason) => write!(f, "cannot serialize frame: {reason}"),
        }
    }
}

fn serialize(wrapper: &WireWrapper) -> Result<Vec<u8>, EncodeError> {
    bincode2::serialize(wrapper).map_err(|err| EncodeError::Serialize(err.to_string()))
}

/// The request that carries `frame` to its peer through the internal service.
pub fn to_request(
    frame: OutboundFrame<WrappedMessage>,
) -> Result<InternalServiceRequest, EncodeError> {
    let OutboundFrame {
        payload,
        extensions,
    } = frame;
    match payload {
        Payload::Message(message) => data_request(message, extensions),
        control => control_request(control, extensions),
    }
}

fn data_request(
    message: WrappedMessage,
    extensions: FrameExtensions<u64>,
) -> Result<InternalServiceRequest, EncodeError> {
    let WrappedMessage {
        source_id,
        destination_id,
        message_id,
        contents,
    } = message;
    let InternalServicePayload::Request(InternalServiceRequest::Message {
        request_id,
        message: raw,
        cid,
        peer_cid,
        security_level,
    }) = contents
    else {
        return Err(EncodeError::Unencodable(format!(
            "data frame {message_id} for {destination_id} is not an outgoing message"
        )));
    };
    let wrapper = match extensions {
        FrameExtensions::None => WireWrapper::Message {
            source: source_id,
            destination: destination_id,
            message_id,
            contents: raw,
        },
        FrameExtensions::Negotiated {
            piggybacked_ack,
            compression,
        } => {
            // The one policy table decides, on the real size of the bytes.
            let encoded = match compression {
                Some(plan) => compression::encode(Some(plan.hint), plan.codecs, raw)
                    .map_err(EncodeError::Compression)?,
                None => compression::Encoded {
                    codec: Codec::Identity,
                    bytes: raw,
                },
            };
            if encoded.codec == Codec::Identity && piggybacked_ack.is_none() {
                // Compression did not pay and nothing else rides along: the
                // legacy frame is smaller and says exactly the same thing.
                WireWrapper::Message {
                    source: source_id,
                    destination: destination_id,
                    message_id,
                    contents: encoded.bytes,
                }
            } else {
                WireWrapper::MessageV2 {
                    source: source_id,
                    destination: destination_id,
                    message_id,
                    codec: encoded.codec.to_wire(),
                    dict_id: 0,
                    piggybacked_ack,
                    contents: encoded.bytes,
                }
            }
        }
        FrameExtensions::Advertise(_) => {
            return Err(EncodeError::Unencodable(format!(
                "data frame {message_id} carries an advertisement; only Ack and Poll may"
            )))
        }
    };
    Ok(InternalServiceRequest::Message {
        request_id,
        message: serialize(&wrapper)?,
        cid,
        peer_cid,
        security_level,
    })
}

fn control_request(
    control: InternalMessage,
    extensions: FrameExtensions<u64>,
) -> Result<InternalServiceRequest, EncodeError> {
    let cid = control.source_id();
    let peer_cid = control.destination_id();
    let advertised = match extensions {
        FrameExtensions::None => None,
        FrameExtensions::Advertise(capabilities) => Some(capabilities),
        FrameExtensions::Negotiated { .. } => {
            return Err(EncodeError::Unencodable(
                "an Ack or Poll carries data-frame extensions".to_string(),
            ))
        }
    };
    let mut bytes = serialize(&WireWrapper::ISMAux {
        signal: Box::new(control),
    })?;
    if let Some(capabilities) = advertised {
        advertisement::append(&mut bytes, capabilities);
    }
    Ok(InternalServiceRequest::Message {
        request_id: Uuid::new_v4(),
        message: bytes,
        cid,
        peer_cid: Some(peer_cid),
        security_level: Default::default(),
    })
}
