//! A double of the agent for the conversation store's tests: a map for
//! LocalDB, a list for what windows were told, a list for what went to the peer.

use super::command::{self, Envelope, Inbound};
use super::engine::store;
use super::io::ConversationIo;
use super::kv::{ConversationKv, KvResult, MemoryKv};
use super::outbound::Outgoing;
use citadel_internal_service_types::{
    ConversationEventKind, InternalServiceResponse, MessageNotification, MessageStatus, MessageType,
};
use futures::future::BoxFuture;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub(super) const ME: u64 = 1001;
pub(super) const PEER: u64 = 2002;

#[derive(Default)]
pub(super) struct FakeAgent {
    pub kv: YieldingKv,
    pub published: Mutex<Vec<InternalServiceResponse>>,
    pub sent: Mutex<Vec<Vec<u8>>>,
    pub windows: AtomicBool,
    pub link_down: AtomicBool,
    pub known: AtomicBool,
    pub ids: AtomicU64,
}

/// MemoryKv that yields to the scheduler on every call, so concurrent writers
/// genuinely interleave at each await -- which is what the lock must survive.
#[derive(Default)]
pub(super) struct YieldingKv(pub MemoryKv, pub AtomicBool);

impl ConversationKv for YieldingKv {
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, KvResult<Option<Vec<u8>>>> {
        Box::pin(async move {
            tokio::task::yield_now().await;
            if self.1.load(Ordering::SeqCst) {
                return Err("the disk is unwell".to_string());
            }
            self.0.get(key).await
        })
    }
    fn set<'a>(&'a self, key: &'a str, value: Vec<u8>) -> BoxFuture<'a, KvResult<()>> {
        Box::pin(async move {
            tokio::task::yield_now().await;
            self.0.set(key, value).await
        })
    }
    fn delete<'a>(&'a self, key: &'a str) -> BoxFuture<'a, KvResult<()>> {
        Box::pin(async move { self.0.delete(key).await })
    }
    fn keys(&self) -> BoxFuture<'_, KvResult<Vec<String>>> {
        Box::pin(async move { self.0.keys().await })
    }
}

impl ConversationIo for FakeAgent {
    fn kv(&self) -> &dyn ConversationKv {
        &self.kv
    }
    fn publish(&self, _cid: u64, response: InternalServiceResponse) -> usize {
        self.published.lock().push(response);
        usize::from(self.windows.load(Ordering::SeqCst))
    }
    fn send_p2p(
        &self,
        _cid: u64,
        _peer: u64,
        bytes: Vec<u8>,
    ) -> BoxFuture<'static, Result<(), String>> {
        let down = self.link_down.load(Ordering::SeqCst);
        if !down {
            self.sent.lock().push(bytes);
        }
        Box::pin(async move {
            if down {
                Err("no link to the peer".to_string())
            } else {
                Ok(())
            }
        })
    }
    fn now_ms(&self) -> f64 {
        1790000000000.0
    }
    fn new_id(&self) -> String {
        format!("id-{}", self.ids.fetch_add(1, Ordering::SeqCst))
    }
    fn account_username(&self, _cid: u64) -> String {
        "alice".to_string()
    }
    fn peer_username(&self, _cid: u64, _peer: u64) -> Option<String> {
        Some("bob".to_string())
    }
    fn knows_peer(&self, _cid: u64, _peer: u64) -> BoxFuture<'static, bool> {
        let known = self.known.load(Ordering::SeqCst);
        Box::pin(async move { known })
    }
}

impl FakeAgent {
    pub fn with_window() -> Self {
        let agent = Self::default();
        agent.windows.store(true, Ordering::SeqCst);
        agent.known.store(true, Ordering::SeqCst);
        agent
    }
    pub fn events(&self) -> Vec<(ConversationEventKind, Option<String>, String)> {
        self.published
            .lock()
            .iter()
            .filter_map(|r| match r {
                InternalServiceResponse::ConversationEvent(e) => Some((
                    e.kind,
                    e.message.as_ref().map(|m| m.id.clone()),
                    e.preview.clone(),
                )),
                _ => None,
            })
            .collect()
    }
    pub fn sent_commands(&self) -> Vec<Inbound> {
        self.sent
            .lock()
            .iter()
            .filter_map(|b| command::read(b))
            .collect()
    }
}

pub(super) fn from_peer(bytes: Vec<u8>) -> MessageNotification {
    MessageNotification {
        message: bytes,
        cid: ME,
        peer_cid: PEER,
        request_id: None,
    }
}

pub(super) fn chat(id: &str, text: &str) -> Vec<u8> {
    let envelope = Envelope {
        sender_cid: PEER,
        recipient_cid: ME,
        message_id: id.into(),
        index: 1.0,
        reply_to: None,
        mentions: None,
        attachments: None,
        message_type: MessageType::Text,
        document_id: None,
        document_title: None,
    };
    command::messaging_layer(command::message_layer(text, 5.0), &envelope)
}

pub(super) fn outgoing(text: &str) -> Outgoing {
    Outgoing {
        content: text.into(),
        message_type: MessageType::Text,
        reply_to: None,
        mentions: None,
        attachments: None,
        document_id: None,
        document_title: None,
    }
}

pub(super) async fn status_of(agent: &FakeAgent, id: &str) -> Option<MessageStatus> {
    let (_, page, index) = store(agent, ME).find(PEER, id).await.unwrap()?;
    Some(page.messages[index].status)
}
