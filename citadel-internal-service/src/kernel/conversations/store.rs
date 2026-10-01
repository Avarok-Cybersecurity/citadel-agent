//! One account's view of its conversation records: where each lives, how it
//! loads, and appending.
//!
//! Ported from the web UI's message-pagination-store.ts and the page operations
//! it calls, which it replaces as the single writer. Every function assumes the
//! caller holds the conversation's lock (engine.rs); none of them locks.

use super::kv::{ConversationKv, KvResult};
use super::stored;
use citadel_internal_service_types::{ConversationMessage, ConversationMetadata, ConversationPage};

pub(crate) const PAGINATED_PREFIX: &str = "msgs_with_peer_";
pub(crate) const MESSAGES_PER_PAGE: usize = 50;

pub(crate) struct Store<'a> {
    pub(crate) kv: &'a dyn ConversationKv,
    /// The account whose conversations these are.
    pub(crate) own: u64,
}

/// Keys are scoped by the owning account; records written before that are
/// under the peer alone, and are still read -- but only an unstamped one or
/// one stamped for this account.
pub(crate) fn scoped(own: u64, peer: u64) -> String {
    format!("{PAGINATED_PREFIX}{own}_with_{peer}")
}

pub(crate) fn legacy(peer: u64) -> String {
    format!("{PAGINATED_PREFIX}{peer}")
}

fn stored_err(e: stored::StoredError) -> String {
    e.0
}

impl Store<'_> {
    async fn metadata_at(&self, key: &str) -> KvResult<Option<ConversationMetadata>> {
        match self.kv.get(key).await? {
            Some(bytes) => stored::decode_metadata(&bytes)
                .map(Some)
                .map_err(stored_err),
            None => Ok(None),
        }
    }

    async fn page_at(&self, key: &str) -> KvResult<Option<ConversationPage>> {
        match self.kv.get(key).await? {
            Some(bytes) => stored::decode_page(&bytes).map(Some).map_err(stored_err),
            None => Ok(None),
        }
    }

    pub(crate) async fn load_metadata(&self, peer: u64) -> KvResult<Option<ConversationMetadata>> {
        if let Some(found) = self
            .metadata_at(&format!("{}_metadata", scoped(self.own, peer)))
            .await?
        {
            return Ok(Some(found));
        }
        let legacy = self
            .metadata_at(&format!("{}_metadata", legacy(peer)))
            .await?;
        Ok(legacy.filter(|m| m.owner_cid.is_none_or(|owner| owner == self.own)))
    }

    pub(crate) async fn load_page(
        &self,
        peer: u64,
        number: u32,
    ) -> KvResult<Option<ConversationPage>> {
        if let Some(page) = self
            .page_at(&format!("{}_{number}", scoped(self.own, peer)))
            .await?
        {
            return Ok(Some(page));
        }
        self.page_at(&format!("{}_{number}", legacy(peer))).await
    }

    pub(crate) async fn save_metadata(&self, metadata: &ConversationMetadata) -> KvResult<()> {
        let key = format!("{}_metadata", scoped(self.own, metadata.peer_cid));
        self.kv
            .set(&key, stored::encode_metadata(metadata).map_err(stored_err)?)
            .await
    }

    pub(crate) async fn save_page(&self, page: &ConversationPage) -> KvResult<()> {
        let key = format!("{}_{}", scoped(self.own, page.peer_cid), page.page_number);
        self.kv
            .set(&key, stored::encode_page(page).map_err(stored_err)?)
            .await
    }

    /// Append `message`, unless it is already on the newest page or the one
    /// before it (a redelivery). `Ok(None)` is that duplicate; `Ok(Some)` is
    /// the metadata after the append.
    pub(crate) async fn append(
        &self,
        peer: u64,
        message: &ConversationMessage,
        peer_username: Option<String>,
        now: f64,
    ) -> KvResult<Option<ConversationMetadata>> {
        let existing = self.load_metadata(peer).await?;
        let is_new = existing.is_none();
        let mut metadata = existing.unwrap_or(ConversationMetadata {
            peer_cid: peer,
            owner_cid: Some(self.own),
            peer_username,
            total_message_count: 0.0,
            oldest_message_timestamp: message.timestamp,
            newest_message_timestamp: message.timestamp,
            latest_page: 0,
            messages_per_page: MESSAGES_PER_PAGE as u32,
            unread_count: 0.0,
            last_message_index: 0.0,
            last_updated: now,
        });
        // Adopt an unstamped record the first time this account writes to it.
        metadata.owner_cid.get_or_insert(self.own);

        let mut page = self
            .load_page(peer, metadata.latest_page)
            .await?
            .unwrap_or_else(|| empty_page(peer, metadata.latest_page, message.timestamp));

        // Before the rollover: rolling over replaces the page with an empty one,
        // and a duplicate arriving as a page filled would be compared to nothing.
        if self
            .already_stored(peer, &metadata, &page, &message.id)
            .await?
        {
            return Ok(None);
        }
        if page.messages.len() >= MESSAGES_PER_PAGE {
            self.save_page(&page).await?;
            metadata.latest_page += 1;
            page = empty_page(peer, metadata.latest_page, message.timestamp);
        }

        place(&mut page, message.clone());
        record_append(&mut metadata, message, is_new, self.own, now);
        // Page first, pointer last: the metadata is the only pointer to the page.
        self.save_page(&page).await?;
        self.save_metadata(&metadata).await?;
        Ok(Some(metadata))
    }

    async fn already_stored(
        &self,
        peer: u64,
        metadata: &ConversationMetadata,
        page: &ConversationPage,
        id: &str,
    ) -> KvResult<bool> {
        if page.messages.iter().any(|m| m.id == id) {
            return Ok(true);
        }
        if metadata.latest_page == 0 {
            return Ok(false);
        }
        let previous = self.load_page(peer, metadata.latest_page - 1).await?;
        Ok(previous.is_some_and(|p| p.messages.iter().any(|m| m.id == id)))
    }

    /// The newest-first search every per-message change starts with.
    pub(crate) async fn find(
        &self,
        peer: u64,
        id: &str,
    ) -> KvResult<Option<(ConversationMetadata, ConversationPage, usize)>> {
        let Some(metadata) = self.load_metadata(peer).await? else {
            return Ok(None);
        };
        for number in (0..=metadata.latest_page).rev() {
            if let Some(page) = self.load_page(peer, number).await? {
                if let Some(index) = page.messages.iter().position(|m| m.id == id) {
                    return Ok(Some((metadata, page, index)));
                }
            }
        }
        Ok(None)
    }

    /// Change one stored message with `change`, which returns whether it did.
    pub(crate) async fn update(
        &self,
        peer: u64,
        id: &str,
        change: impl FnOnce(&mut ConversationMessage) -> bool,
    ) -> KvResult<Option<ConversationMessage>> {
        let Some((_, mut page, index)) = self.find(peer, id).await? else {
            return Ok(None);
        };
        if !change(&mut page.messages[index]) {
            return Ok(None);
        }
        let changed = page.messages[index].clone();
        self.save_page(&page).await?;
        Ok(Some(changed))
    }
}

pub(crate) use super::store_rules::status_advances;
use super::store_rules::{empty_page, place, record_append};
