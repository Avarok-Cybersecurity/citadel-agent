//! The single writer: one lock per conversation, one event sequence per account.

use super::io::ConversationIo;
use super::kv::KvResult;
use super::store::Store;
use citadel_internal_service_types::{
    AccountPreferences, ConversationEvent, ConversationEventKind, ConversationMessage,
    ConversationMetadata, InternalServiceResponse,
};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::OwnedMutexGuard;
use uuid::Uuid;

/// How much of a message an event carries for a notification to show.
const PREVIEW_CHARS: usize = 100;

/// One lock per (account, peer) conversation.
type ConversationLocks = Mutex<HashMap<(u64, u64), Arc<tokio::sync::Mutex<()>>>>;

#[derive(Default)]
pub(crate) struct Engine {
    locks: ConversationLocks,
    seq: Mutex<HashMap<u64, u64>>,
}

/// What one event says, before the engine numbers and addresses it.
pub(crate) struct Change {
    pub kind: ConversationEventKind,
    pub message: Option<ConversationMessage>,
    pub message_id: Option<String>,
    pub metadata: Option<ConversationMetadata>,
    /// The request that caused it, for the window that asked (its send's bubble).
    pub request_id: Option<Uuid>,
}

impl Change {
    pub(crate) fn message(kind: ConversationEventKind, message: ConversationMessage) -> Self {
        Self {
            kind,
            message_id: Some(message.id.clone()),
            message: Some(message),
            metadata: None,
            request_id: None,
        }
    }
}

pub(crate) fn store<'a>(io: &'a dyn ConversationIo, cid: u64) -> Store<'a> {
    Store {
        kv: io.kv(),
        own: cid,
    }
}

impl Engine {
    /// Held across a whole read-modify-write of one conversation, by every
    /// path that writes it: requests from any window and arriving messages.
    pub(crate) async fn lock(&self, cid: u64, peer: u64) -> OwnedMutexGuard<()> {
        let lock = self.locks.lock().entry((cid, peer)).or_default().clone();
        lock.lock_owned().await
    }

    /// Tell every window of `cid` what changed in its conversation with `peer`.
    pub(crate) async fn announce(
        &self,
        io: &dyn ConversationIo,
        cid: u64,
        peer: u64,
        change: Change,
    ) {
        let metadata = match change.metadata {
            Some(m) => Some(m),
            None => store(io, cid).load_metadata(peer).await.ok().flatten(),
        };
        let seq = {
            let mut seq = self.seq.lock();
            let next = seq.entry(cid).or_insert(0);
            *next += 1;
            *next
        };
        let preview = match (&change.kind, &change.message) {
            (ConversationEventKind::Appended, Some(m)) => {
                m.content.chars().take(PREVIEW_CHARS).collect()
            }
            _ => String::new(),
        };
        let event = ConversationEvent {
            cid,
            peer_cid: peer,
            seq,
            kind: change.kind,
            message: change.message,
            message_id: change.message_id,
            peer_username: metadata.as_ref().and_then(|m| m.peer_username.clone()),
            metadata,
            account_username: io.account_username(cid),
            preview,
            request_id: change.request_id,
        };
        io.publish(
            cid,
            InternalServiceResponse::ConversationEvent(Box::new(event)),
        );
        io.rows_changed();
    }
}

const PREFERENCES_KEY: &str = "agent_account_preferences_";

/// The account's preferences, or the web UI's defaults if it never pushed any.
pub(crate) async fn preferences(io: &dyn ConversationIo, cid: u64) -> KvResult<AccountPreferences> {
    match io.kv().get(&format!("{PREFERENCES_KEY}{cid}")).await? {
        Some(bytes) => serde_json::from_slice(&bytes).map_err(|e| e.to_string()),
        None => Ok(AccountPreferences::UI_DEFAULTS),
    }
}

pub(crate) async fn save_preferences(
    io: &dyn ConversationIo,
    cid: u64,
    preferences: &AccountPreferences,
) -> KvResult<()> {
    let bytes = serde_json::to_vec(preferences).map_err(|e| e.to_string())?;
    io.kv().set(&format!("{PREFERENCES_KEY}{cid}"), bytes).await
}
