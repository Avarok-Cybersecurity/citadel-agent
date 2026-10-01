//! A window's own actions in a conversation, carried out by the agent.
//!
//! Ported from the web UI's message-sender.ts, resend-message.ts,
//! messenger-revision.ts, messenger-reaction.ts and messenger-compatibility.ts
//! (`markMessagesAsRead`): the same ids, indexes, statuses and P2P commands, but
//! stored by the one writer, so two windows acting at once cannot lose a write.

use super::command::{self, Envelope};
use super::engine::{store, Change, Engine};
use super::envelope::{raw_envelope, received};
use super::io::ConversationIo;
use super::store_mutations::{Revised, Revision};
use citadel_internal_service_types::{
    Attachment, ConversationEventKind, ConversationMessage, MessageStatus, MessageType,
};

/// What `ConversationSend` carries, minus the addressing.
pub(crate) struct Outgoing {
    pub content: String,
    pub message_type: MessageType,
    pub reply_to: Option<String>,
    pub mentions: Option<Vec<String>>,
    pub attachments: Option<Vec<Attachment>>,
    pub document_id: Option<String>,
    pub document_title: Option<String>,
}

pub(crate) type Outcome = Result<Option<ConversationMessage>, String>;

impl Engine {
    /// Number it, store it pending, send it, store how that went.
    pub(crate) async fn send(
        &self,
        io: &dyn ConversationIo,
        cid: u64,
        peer: u64,
        out: Outgoing,
    ) -> Outcome {
        let _held = self.lock(cid, peer).await;
        let s = store(io, cid);
        let index = s
            .load_metadata(peer)
            .await?
            .map_or(0.0, |m| m.last_message_index)
            + 1.0;
        let envelope = Envelope {
            sender_cid: cid,
            recipient_cid: peer,
            message_id: io.new_id(),
            index,
            reply_to: out.reply_to,
            mentions: out.mentions,
            attachments: out.attachments,
            message_type: out.message_type,
            document_id: out.document_id,
            document_title: out.document_title,
        };
        let timestamp = io.now_ms();
        let message = ConversationMessage {
            sender_cid: cid,
            recipient_cid: peer,
            ..received(&envelope, out.content, timestamp)
        };
        if let Some(metadata) = s
            .append(peer, &message, io.peer_username(cid, peer), io.now_ms())
            .await?
        {
            let mut change = Change::message(ConversationEventKind::Appended, message.clone());
            change.metadata = Some(metadata);
            self.announce(io, cid, peer, change).await;
        }
        self.transmit(io, cid, peer, &message, &envelope).await
    }

    /// A failed message again: same id, index, time and content.
    pub(crate) async fn resend(
        &self,
        io: &dyn ConversationIo,
        cid: u64,
        peer: u64,
        id: &str,
    ) -> Outcome {
        let _held = self.lock(cid, peer).await;
        let s = store(io, cid);
        let Some((_, page, index)) = s.find(peer, id).await? else {
            return Err(format!("No message {id} in this conversation"));
        };
        let message = page.messages[index].clone();
        if message.status != MessageStatus::Failed || message.sender_cid != cid {
            return Err("Only a failed message you sent can be sent again".to_string());
        }
        if let Some(pending) = s.set_status(peer, id, MessageStatus::Pending, None).await? {
            self.announce(
                io,
                cid,
                peer,
                Change::message(ConversationEventKind::Updated, pending),
            )
            .await;
        }
        let envelope = Envelope {
            sender_cid: cid,
            recipient_cid: peer,
            message_id: message.id.clone(),
            index: message.index,
            reply_to: message.reply_to.clone(),
            mentions: message.mentions.clone(),
            attachments: message.attachments.clone(),
            message_type: message.message_type,
            document_id: message.document_id.clone(),
            document_title: message.document_title.clone(),
        };
        self.transmit(io, cid, peer, &message, &envelope).await
    }

    /// Send `message` and record `sent` or `failed`; the caller holds the lock.
    async fn transmit(
        &self,
        io: &dyn ConversationIo,
        cid: u64,
        peer: u64,
        message: &ConversationMessage,
        envelope: &Envelope,
    ) -> Outcome {
        let bytes = command::messaging_layer(
            command::message_layer(&message.content, message.timestamp),
            envelope,
        );
        let (status, error) = match io.send_p2p(cid, peer, bytes).await {
            Ok(()) => (MessageStatus::Sent, None),
            Err(e) => (MessageStatus::Failed, Some(e)),
        };
        let updated = store(io, cid)
            .set_status(peer, &message.id, status, error.clone())
            .await?;
        if let Some(m) = &updated {
            self.announce(
                io,
                cid,
                peer,
                Change::message(ConversationEventKind::Updated, m.clone()),
            )
            .await;
        }
        match error {
            Some(e) => Err(e),
            None => Ok(updated),
        }
    }

    /// An edit or delete of the account's own message: stored, then sent.
    pub(crate) async fn revise_own(
        &self,
        io: &dyn ConversationIo,
        cid: u64,
        peer: u64,
        id: &str,
        edit: Option<String>,
    ) -> Outcome {
        let _held = self.lock(cid, peer).await;
        let now = io.now_ms();
        let (revision, layer, kind) = match edit {
            Some(contents) => (
                Revision::Edit {
                    contents: contents.clone(),
                    edited_at: now,
                },
                command::edit_layer(id, &contents, now),
                ConversationEventKind::Updated,
            ),
            None => (
                Revision::Delete,
                command::delete_layer(id, now),
                ConversationEventKind::Removed,
            ),
        };
        let message = match store(io, cid).revise(peer, id, cid, revision, now).await? {
            Revised::Applied(m) => *m,
            Revised::NotSender => return Err("Only the sender may change a message".to_string()),
            Revised::Unknown => return Err(format!("No message {id} in this conversation")),
        };
        self.announce(io, cid, peer, Change::message(kind, message.clone()))
            .await;
        let envelope = raw_envelope(io, cid, peer).await;
        io.send_p2p(cid, peer, command::messaging_layer(layer, &envelope))
            .await
            .map_err(|e| format!("Saved here, but not sent: {e}"))?;
        Ok(Some(message))
    }
}
