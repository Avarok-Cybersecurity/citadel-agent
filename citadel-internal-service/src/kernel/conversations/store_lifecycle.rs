//! A conversation's whole-record changes: deleting it, expiring old messages,
//! and listing an account's conversations. Ported from message-page-delete.ts,
//! retention.ts and load-all-metadata.ts.

use super::kv::KvResult;
use super::store::{legacy, scoped, Store, PAGINATED_PREFIX};
use super::stored;
use citadel_internal_service_types::ConversationMetadata;

impl Store<'_> {
    /// Delete the conversation, if this account may: a record stamped for it,
    /// or an unstamped one when `include_unattributed`. Both key shapes go.
    pub(crate) async fn delete(&self, peer: u64, include_unattributed: bool) -> KvResult<bool> {
        let may = |m: &ConversationMetadata| match m.owner_cid {
            None => include_unattributed,
            Some(owner) => owner == self.own,
        };
        let Some(metadata) = self.load_metadata(peer).await? else {
            return Ok(false);
        };
        if !may(&metadata) {
            return Ok(false);
        }
        let scoped_prefix = scoped(self.own, peer);
        for number in 0..=metadata.latest_page {
            self.kv.delete(&format!("{scoped_prefix}_{number}")).await?;
        }
        self.kv.delete(&format!("{scoped_prefix}_metadata")).await?;
        let legacy_prefix = legacy(peer);
        let legacy_meta = match self.kv.get(&format!("{legacy_prefix}_metadata")).await? {
            Some(bytes) => Some(stored::decode_metadata(&bytes).map_err(|e| e.0)?),
            None => None,
        };
        let (legacy_pages, allowed) = match &legacy_meta {
            Some(m) => (m.latest_page, may(m)),
            None => (metadata.latest_page, include_unattributed),
        };
        if allowed {
            for number in 0..=legacy_pages {
                self.kv.delete(&format!("{legacy_prefix}_{number}")).await?;
            }
            self.kv.delete(&format!("{legacy_prefix}_metadata")).await?;
        }
        Ok(true)
    }

    /// Remove messages older than `cutoff`; the number removed.
    pub(crate) async fn prune_older_than(
        &self,
        peer: u64,
        cutoff: f64,
        now: f64,
    ) -> KvResult<usize> {
        let Some(mut metadata) = self.load_metadata(peer).await? else {
            return Ok(0);
        };
        if metadata.total_message_count == 0.0
            || (metadata.oldest_message_timestamp > 0.0
                && metadata.oldest_message_timestamp >= cutoff)
        {
            return Ok(0);
        }
        let (mut removed, mut oldest_kept) = (0usize, None::<f64>);
        for number in 0..=metadata.latest_page {
            let Some(mut page) = self.load_page(peer, number).await? else {
                continue;
            };
            let before = page.messages.len();
            page.messages.retain(|m| m.timestamp >= cutoff);
            for m in &page.messages {
                oldest_kept = Some(oldest_kept.map_or(m.timestamp, |o| o.min(m.timestamp)));
            }
            if page.messages.len() == before {
                continue;
            }
            removed += before - page.messages.len();
            page.page_timestamps.min_timestamp = page.messages.first().map_or(0.0, |m| m.timestamp);
            page.page_timestamps.max_timestamp = page.messages.last().map_or(0.0, |m| m.timestamp);
            self.save_page(&page).await?;
        }
        if removed == 0 {
            return Ok(0);
        }
        metadata.total_message_count = (metadata.total_message_count - removed as f64).max(0.0);
        metadata.oldest_message_timestamp = oldest_kept.unwrap_or(0.0);
        metadata.last_updated = now;
        self.save_metadata(&metadata).await?;
        Ok(removed)
    }

    /// Every conversation of this account: its own records and unstamped ones,
    /// the account-scoped record winning over a legacy one for the same peer.
    pub(crate) async fn list(&self) -> KvResult<Vec<ConversationMetadata>> {
        let mut found: Vec<ConversationMetadata> = Vec::new();
        let mut keys: Vec<String> = self
            .kv
            .keys()
            .await?
            .into_iter()
            .filter(|k| k.starts_with(PAGINATED_PREFIX) && k.ends_with("_metadata"))
            .collect();
        // Scoped keys (with `_with_`) first, so they win the de-duplication.
        keys.sort_by_key(|k| !k.contains("_with_"));
        for key in keys {
            let Some(bytes) = self.kv.get(&key).await? else {
                continue;
            };
            let Ok(metadata) = stored::decode_metadata(&bytes) else {
                continue;
            };
            let ours = metadata.owner_cid.is_none_or(|owner| owner == self.own);
            let scoped_to_other =
                key.contains("_with_") && !key.starts_with(&scoped(self.own, metadata.peer_cid));
            if ours && !scoped_to_other && !found.iter().any(|m| m.peer_cid == metadata.peer_cid) {
                found.push(metadata);
            }
        }
        Ok(found)
    }
}
