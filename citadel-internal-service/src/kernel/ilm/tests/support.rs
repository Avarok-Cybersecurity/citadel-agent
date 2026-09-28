//! In-memory stand-ins for the two things an agent-hosted ILM touches outside
//! itself: the account's LocalDB and the session's peer channels.
//!
//! Justification for each (they replace I/O, not logic): `MemoryDb` stands in
//! for the SDK persistence handler, which needs a running node; `Loopback`
//! stands in for `conn.peers[peer].sink`, which needs a live P2P channel. What
//! runs over them is production code: the shared backend, the shared frame
//! encoder/decoder, the agent transport, and the real ILM.
use crate::kernel::ilm::host::{AgentIlmHost, FeedError};
use crate::kernel::ilm::kv::LocalDbAccess;
use crate::kernel::ilm::transport::IlmPeerLinks;
use citadel_internal_service_types::{MessageNotification, SecurityLevel};
use citadel_sdk::prelude::async_trait;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

/// `(session_cid, key)` -> value.
pub type Entries = BTreeMap<(u64, String), Vec<u8>>;

/// One account namespace per CID, keys stored verbatim -- the shape of the
/// SDK's `(session_cid, peer_cid = 0, key)` byte map.
#[derive(Clone, Default)]
pub struct MemoryDb {
    entries: Arc<Mutex<Entries>>,
}

impl MemoryDb {
    pub fn snapshot(&self) -> Entries {
        self.entries.lock().unwrap().clone()
    }
}

#[async_trait]
impl LocalDbAccess for MemoryDb {
    async fn get(&self, cid: u64, key: &str) -> Result<Option<Vec<u8>>, String> {
        Ok(self
            .entries
            .lock()
            .unwrap()
            .get(&(cid, key.to_string()))
            .cloned())
    }

    async fn set(&self, cid: u64, key: &str, value: Vec<u8>) -> Result<(), String> {
        self.entries
            .lock()
            .unwrap()
            .insert((cid, key.to_string()), value);
        Ok(())
    }
}

/// A frame as it crossed the stand-in wire.
#[derive(Clone, Debug)]
pub struct Sent {
    pub from: u64,
    pub to: u64,
    pub bytes: Vec<u8>,
}

/// Peer channels between hosts in one process. A frame sent from `cid` to
/// `peer` arrives at `peer` as the `MessageNotification` the receiving agent
/// builds in `responses/peer_channel_created.rs`: recipient in `cid`, sender
/// in `peer_cid`, frame bytes as the body.
#[derive(Clone, Default)]
pub struct Loopback {
    routes: Arc<Mutex<HashMap<u64, UnboundedSender<MessageNotification>>>>,
    pub sent: Arc<Mutex<Vec<Sent>>>,
}

impl Loopback {
    /// Pumps everything addressed to the host's CID into `host.feed`.
    pub fn attach(&self, host: Arc<AgentIlmHost<MemoryDb, Loopback>>) {
        let (tx, mut rx) = unbounded_channel::<MessageNotification>();
        self.routes.lock().unwrap().insert(host.cid(), tx);
        drop(tokio::spawn(async move {
            while let Some(notification) = rx.recv().await {
                match host.feed(notification) {
                    Ok(()) => {}
                    Err(FeedError::Stopped) => return,
                    Err(FeedError::NotAFrame(n)) => panic!("not an ILM frame: {:?}", n.message),
                }
            }
        }));
    }
}

#[async_trait]
impl IlmPeerLinks for Loopback {
    async fn send_to_peer(
        &self,
        cid: u64,
        peer_cid: u64,
        _security_level: SecurityLevel,
        frame: Vec<u8>,
    ) -> Result<(), String> {
        self.sent.lock().unwrap().push(Sent {
            from: cid,
            to: peer_cid,
            bytes: frame.clone(),
        });
        let route = self.routes.lock().unwrap().get(&peer_cid).cloned();
        let route = route.ok_or_else(|| format!("Peer connection for {peer_cid} not found"))?;
        route
            .send(MessageNotification {
                message: frame,
                cid: peer_cid,
                peer_cid: cid,
                request_id: None,
            })
            .map_err(|_| format!("peer {peer_cid} is gone"))
    }

    fn connected_peers(&self, cid: u64) -> Vec<u64> {
        let mut peers: Vec<u64> = self
            .routes
            .lock()
            .unwrap()
            .keys()
            .copied()
            .filter(|peer| *peer != cid)
            .collect();
        peers.sort_unstable();
        peers
    }
}
