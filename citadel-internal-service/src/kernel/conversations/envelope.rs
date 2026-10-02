//! Messages and envelopes built from one another.

use super::command::Envelope;
use super::engine::store;
use super::io::ConversationIo;
use citadel_internal_service_types::{ConversationMessage, MessageStatus, MessageType};

/// A message built from an envelope; the caller sets sender, recipient, status.
pub(crate) fn received(
    envelope: &Envelope,
    content: String,
    timestamp: f64,
) -> ConversationMessage {
    ConversationMessage {
        id: envelope.message_id.clone(),
        content,
        sender_cid: envelope.sender_cid,
        recipient_cid: envelope.recipient_cid,
        timestamp,
        index: envelope.index,
        status: MessageStatus::Pending,
        error: None,
        reply_to: envelope.reply_to.clone(),
        edited_at: None,
        reactions: None,
        mentions: envelope.mentions.clone(),
        attachments: envelope.attachments.clone(),
        message_type: envelope.message_type,
        document_id: envelope.document_id.clone(),
        document_title: envelope.document_title.clone(),
        transfer_id: None,
        file_name: None,
        file_size: None,
        file_type: None,
        file_thumbnail: None,
        transfer_mode: None,
        transfer_state: None,
        transfer_progress: None,
        virtual_path: None,
    }
}

/// The envelope a non-chat command goes out in (`sendRawMessage`): a fresh id
/// and the conversation's current index, not incremented.
pub(crate) async fn raw_envelope(io: &dyn ConversationIo, cid: u64, peer: u64) -> Envelope {
    let index = store(io, cid)
        .load_metadata(peer)
        .await
        .ok()
        .flatten()
        .map_or(0.0, |m| m.last_message_index);
    Envelope {
        sender_cid: cid,
        recipient_cid: peer,
        message_id: io.new_id(),
        index,
        reply_to: None,
        mentions: None,
        attachments: None,
        message_type: MessageType::Text,
        document_id: None,
        document_title: None,
    }
}
