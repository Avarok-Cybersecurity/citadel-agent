//! ILM frames on the P2P wire: the ONE encoder and decoder.
//!
//! Every ILM payload crossing between peers is a bincode2 [`WireWrapper`] in
//! the body of a P2P message. A `Payload::Message` travels as
//! `WireWrapper::Message` carrying the application's bytes; ACKs and POLLs
//! travel as `WireWrapper::ISMAux` carrying the whole payload. The receiver
//! decodes whatever the sender encoded, and a browser-hosted ILM and an
//! agent-hosted one talk to each other across machines, so the two hosts must
//! frame identically. They do by sharing these functions -- the browser's
//! messenger (`mod.rs`) and the agent's transport both call them.
use crate::messenger::{InternalMessage, WireWrapper, WrappedMessage};
use citadel_internal_service_types::{
    InternalServicePayload, InternalServiceRequest, InternalServiceResponse, MessageNotification,
    SecurityLevel,
};
use intersession_layer_messaging::Payload;
use uuid::Uuid;

/// One ILM payload, framed and addressed, ready to become a P2P message.
#[derive(Debug)]
pub struct OutboundFrame {
    pub request_id: Uuid,
    pub cid: u64,
    pub peer_cid: Option<u64>,
    pub security_level: SecurityLevel,
    pub bytes: Vec<u8>,
}

impl OutboundFrame {
    /// The request the browser sends its agent for this frame.
    pub fn into_request(self) -> InternalServiceRequest {
        InternalServiceRequest::Message {
            request_id: self.request_id,
            message: self.bytes,
            cid: self.cid,
            peer_cid: self.peer_cid,
            security_level: self.security_level,
        }
    }
}

#[derive(Debug)]
pub enum FrameError {
    /// A `Payload::Message` whose contents are not an outgoing
    /// `InternalServiceRequest::Message`: there is nothing to send to a peer.
    NotAMessageRequest(Box<InternalMessage>),
    Serialize(bincode2::Error),
}

/// Frame an ILM payload for a peer.
///
/// `Payload::Message` keeps the original request's id, cid, peer and security
/// level; only its body is wrapped. ACK/POLL get a fresh request id, the
/// payload's source as cid, its destination as peer, and the default security
/// level -- exactly what the messenger has always sent.
pub fn encode_outbound(payload: InternalMessage) -> Result<OutboundFrame, FrameError> {
    match payload {
        Payload::Message(WrappedMessage {
            source_id,
            destination_id,
            message_id,
            contents:
                InternalServicePayload::Request(InternalServiceRequest::Message {
                    request_id,
                    message,
                    cid,
                    peer_cid,
                    security_level,
                }),
        }) => {
            let wire_message = WireWrapper::Message {
                contents: message,
                source: source_id,
                destination: destination_id,
                message_id,
            };
            Ok(OutboundFrame {
                request_id,
                cid,
                peer_cid,
                security_level,
                bytes: bincode2::serialize(&wire_message).map_err(FrameError::Serialize)?,
            })
        }
        not_a_request @ Payload::Message(_) => {
            Err(FrameError::NotAMessageRequest(Box::new(not_a_request)))
        }
        signal => {
            let cid = signal.source_id();
            let peer_cid = signal.destination_id();
            let wire_message = WireWrapper::ISMAux {
                signal: Box::new(signal),
            };
            Ok(OutboundFrame {
                request_id: Uuid::new_v4(),
                cid,
                peer_cid: Some(peer_cid),
                security_level: Default::default(),
                bytes: bincode2::serialize(&wire_message).map_err(FrameError::Serialize)?,
            })
        }
    }
}

/// A decoded ILM frame.
pub enum InboundFrame {
    /// An ACK or POLL (or a non-tracked message) for the local ILM.
    Signal(InternalMessage),
    /// An application message. `payload` is for ILM; `unwrapped` is the same
    /// notification with its body replaced by the application's bytes.
    Message {
        payload: InternalMessage,
        unwrapped: MessageNotification,
    },
}

/// Decode a P2P message body as an ILM frame.
///
/// `Err` hands the notification back untouched: not every P2P message is an
/// ILM frame (Yjs updates and plain messages travel raw, by design).
pub fn decode_inbound(
    mut notification: MessageNotification,
) -> Result<InboundFrame, (bincode2::Error, MessageNotification)> {
    match bincode2::deserialize::<WireWrapper>(&notification.message) {
        Ok(WireWrapper::ISMAux { signal }) => Ok(InboundFrame::Signal(*signal)),
        Ok(WireWrapper::Message {
            source,
            destination,
            message_id,
            contents,
        }) => {
            notification.message = contents;
            let unwrapped = notification.clone();
            let payload = InternalMessage::Message(WrappedMessage {
                source_id: source,
                destination_id: destination,
                message_id,
                contents: InternalServicePayload::Response(
                    InternalServiceResponse::MessageNotification(notification),
                ),
            });
            Ok(InboundFrame::Message { payload, unwrapped })
        }
        Err(err) => Err((err, notification)),
    }
}

#[cfg(test)]
#[path = "wire_tests.rs"]
mod tests;
