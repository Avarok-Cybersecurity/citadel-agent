//! Every change to stored conversations other than appending.
//!
//! Ported from the web UI's message-metadata-mutations.ts, message-page-reaction.ts,
//! message-page-revision.ts, message-page-delete.ts and retention.ts. One
//! deliberate difference: a status change is held to the delivery ladder on
//! disk too. The UI patched a message it did not hold in memory without the
//! check, so a late `delivered` could overwrite `read`.

use super::kv::KvResult;
use super::reactions;
use super::store::{status_advances, Store};

use citadel_internal_service_types::{
    ConversationMessage, ConversationMetadata, MessagePatch, MessageStatus, Reaction,
};

pub(crate) enum Revision {
    Edit { contents: String, edited_at: f64 },
    Delete,
}

#[derive(Debug, PartialEq)]
pub(crate) enum Revised {
    Applied(Box<ConversationMessage>),
    NotSender,
    Unknown,
}

impl Store<'_> {
    pub(crate) async fn set_status(
        &self,
        peer: u64,
        id: &str,
        status: MessageStatus,
        error: Option<String>,
    ) -> KvResult<Option<ConversationMessage>> {
        self.update(peer, id, |m| {
            if !status_advances(m.status, status) {
                return false;
            }
            m.status = status;
            m.error = if status == MessageStatus::Failed {
                error
            } else {
                None
            };
            true
        })
        .await
    }

    /// A window's change to a message: the status on the ladder, the rest as given.
    pub(crate) async fn patch(
        &self,
        peer: u64,
        id: &str,
        patch: MessagePatch,
    ) -> KvResult<Option<ConversationMessage>> {
        self.update(peer, id, |m| {
            let mut changed = false;
            if let Some(status) = patch.status {
                if status_advances(m.status, status) {
                    m.status = status;
                    m.error = patch.error.clone();
                    changed = true;
                }
            }
            macro_rules! set {
                ($field:ident) => {
                    if patch.$field.is_some() && patch.$field != m.$field {
                        m.$field = patch.$field.clone();
                        changed = true;
                    }
                };
            }
            set!(transfer_state);
            set!(transfer_progress);
            set!(virtual_path);
            set!(file_thumbnail);
            changed
        })
        .await
    }

    pub(crate) async fn react(
        &self,
        peer: u64,
        id: &str,
        change: &Reaction,
    ) -> KvResult<Option<ConversationMessage>> {
        self.update(peer, id, |m| {
            match reactions::fold(m.reactions.as_deref(), change) {
                Some(list) => {
                    m.reactions = Some(list);
                    true
                }
                None => false,
            }
        })
        .await
    }

    /// An edit or a delete, by `reviser`, who must be the message's sender.
    pub(crate) async fn revise(
        &self,
        peer: u64,
        id: &str,
        reviser: u64,
        revision: Revision,
        now: f64,
    ) -> KvResult<Revised> {
        let Some((_, mut page, index)) = self.find(peer, id).await? else {
            return Ok(Revised::Unknown);
        };
        let stored = page.messages[index].clone();
        if stored.sender_cid != reviser {
            return Ok(Revised::NotSender);
        }
        match revision {
            Revision::Delete => Ok(if self.remove(peer, id, now).await? {
                Revised::Applied(Box::new(stored))
            } else {
                Revised::Unknown
            }),
            Revision::Edit {
                contents,
                edited_at,
            } => {
                let message = ConversationMessage {
                    content: contents,
                    edited_at: Some(edited_at),
                    ..stored
                };
                page.messages[index] = message.clone();
                self.save_page(&page).await?;
                Ok(Revised::Applied(Box::new(message)))
            }
        }
    }

    pub(crate) async fn remove(&self, peer: u64, id: &str, now: f64) -> KvResult<bool> {
        let Some((mut metadata, mut page, index)) = self.find(peer, id).await? else {
            return Ok(false);
        };
        page.messages.remove(index);
        self.save_page(&page).await?;
        metadata.total_message_count = (metadata.total_message_count - 1.0).max(0.0);
        metadata.last_updated = now;
        self.save_metadata(&metadata).await?;
        Ok(true)
    }

    /// Mark what the peer sent as read: each `delivered` message from it, newest
    /// first, up to the unread count. Returns those messages and the metadata.
    pub(crate) async fn mark_read(
        &self,
        peer: u64,
        now: f64,
    ) -> KvResult<(Vec<ConversationMessage>, Option<ConversationMetadata>)> {
        let Some(mut metadata) = self.load_metadata(peer).await? else {
            return Ok((Vec::new(), None));
        };
        let mut read = Vec::new();
        let wanted = metadata.unread_count.max(0.0) as usize;
        for number in (0..=metadata.latest_page).rev() {
            if read.len() >= wanted {
                break;
            }
            let Some(mut page) = self.load_page(peer, number).await? else {
                continue;
            };
            let mut touched = false;
            for m in page.messages.iter_mut().rev() {
                if read.len() >= wanted {
                    break;
                }
                if m.sender_cid == peer && m.status == MessageStatus::Delivered {
                    m.status = MessageStatus::Read;
                    read.push(m.clone());
                    touched = true;
                }
            }
            if touched {
                self.save_page(&page).await?;
            }
        }
        metadata.unread_count = 0.0;
        metadata.last_updated = now;
        self.save_metadata(&metadata).await?;
        Ok((read, Some(metadata)))
    }

    pub(crate) async fn set_peer_username(
        &self,
        peer: u64,
        username: &str,
        now: f64,
    ) -> KvResult<()> {
        if let Some(mut metadata) = self.load_metadata(peer).await? {
            if metadata.peer_username.as_deref() != Some(username) {
                metadata.peer_username = Some(username.to_string());
                metadata.last_updated = now;
                self.save_metadata(&metadata).await?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
