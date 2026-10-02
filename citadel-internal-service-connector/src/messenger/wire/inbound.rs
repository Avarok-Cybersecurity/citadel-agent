//! A peer's decoded frame as the frame ILM consumes.
//!
//! One construction for every ILM host: the browser's messenger and the agent
//! (multi-window mw3) both feed ILM from `MessageNotification`s, and the frame
//! they build must be the same, or the two would disagree about what was
//! delivered.

use super::Decoded;
use crate::messenger::{InternalMessage, WrappedMessage};
use citadel_internal_service_types::{
    InternalServicePayload, InternalServiceResponse, MessageNotification,
};
use intersession_layer_messaging::{CapabilityEvidence, InboundFrame};

/// The frame for ILM. A data frame carries `notification` with its bytes
/// replaced by the frame's (decompressed) contents: exactly what the sending
/// application handed over, under the session and peer the agent reported.
pub fn into_inbound_frame(
    decoded: Decoded,
    mut notification: MessageNotification,
) -> InboundFrame<WrappedMessage> {
    match decoded {
        Decoded::Control { signal, evidence } => InboundFrame {
            payload: *signal,
            evidence,
            piggybacked_ack: None,
        },
        Decoded::Data {
            contents,
            source,
            destination,
            message_id,
            piggybacked_ack,
        } => {
            notification.message = contents;
            InboundFrame {
                payload: InternalMessage::Message(WrappedMessage {
                    source_id: source,
                    destination_id: destination,
                    message_id,
                    contents: InternalServicePayload::Response(
                        InternalServiceResponse::MessageNotification(notification),
                    ),
                }),
                evidence: CapabilityEvidence::Silent,
                piggybacked_ack,
            }
        }
    }
}

/// The application's message a data frame carries, if it is one.
pub fn carried_notification(frame: &InboundFrame<WrappedMessage>) -> Option<&MessageNotification> {
    match &frame.payload {
        InternalMessage::Message(WrappedMessage {
            contents:
                InternalServicePayload::Response(InternalServiceResponse::MessageNotification(n)),
            ..
        }) => Some(n),
        _ => None,
    }
}
