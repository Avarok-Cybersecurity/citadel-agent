//! A conversation's whole-record changes: deleting it, expiring old messages,
//! and listing an account's conversations. Ported from message-page-delete.ts,
//! retention.ts and load-all-metadata.ts.

use super::kv::KvResult;
use super::store::{own_prefix, scoped, Store};
use super::stored;

impl Store<'_> {
    /// Delete this account's conversation with the peer; false if it has none.
    pub(crate) async fn delete(&self, peer: u64) -> KvResult<bool> {
        let Some(metadata) = self.load_metadata(peer).await? else {
            return Ok(false);
        };
        let scoped_prefix = scoped(self.own, peer);
        for number in 0..=metadata.latest_page {
            self.kv.delete(&format!("{scoped_prefix}_{number}")).await?;
        }
        self.kv.delete(&format!("{scoped_prefix}_metadata")).await?;
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

    /// Every conversation of this account: the records under its own keys.
    pub(crate) async fn list(
        &self,
    ) -> KvResult<Vec<citadel_internal_service_types::ConversationMetadata>> {
        let mut found = Vec::new();
        let prefix = own_prefix(self.own);
        let keys: Vec<String> = self
            .kv
            .keys()
            .await?
            .into_iter()
            .filter(|k| k.starts_with(&prefix) && k.ends_with("_metadata"))
            .collect();
        for key in keys {
            let Some(bytes) = self.kv.get(&key).await? else {
                continue;
            };
            let Ok(metadata) = stored::decode_metadata(&bytes) else {
                continue;
            };
            if metadata.owner_cid == self.own
                && key == format!("{}_metadata", scoped(self.own, metadata.peer_cid))
            {
                found.push(metadata);
            }
        }
        Ok(found)
    }
}
