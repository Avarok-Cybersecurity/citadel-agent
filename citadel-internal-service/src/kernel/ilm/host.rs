//! One ILM per account, inside the agent.
//!
//! Nothing constructs these yet: phase 2b opts sessions in. What is here is the
//! assembly -- the shared `CitadelWorkspaceBackend` over the agent's LocalDB,
//! the peer-sink transport, and the messenger's own `LocalDeliveryTx` -- and a
//! registry that refuses a second ILM for a CID. Two ILMs over one account's
//! keys would each read-modify-write the same queue blobs with separate gates,
//! which is the lost-update bug `backend_map.rs` exists to prevent.
use crate::kernel::ilm::kv::{AgentKvStore, LocalDbAccess};
use crate::kernel::ilm::transport::{AgentIlmTransport, IlmPeerLinks};
use citadel_internal_service_connector::messenger::backend::CitadelWorkspaceBackend;
use citadel_internal_service_connector::messenger::ilm::{BackendError, ILM};
use citadel_internal_service_connector::messenger::wire::{decode_inbound, InboundFrame};
use citadel_internal_service_connector::messenger::{
    InternalMessage, LocalDeliveryTx, WrappedMessage,
};
use citadel_internal_service_types::{InternalServiceResponse, MessageNotification};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;

pub type AgentBackend<D> = CitadelWorkspaceBackend<AgentKvStore<D>>;
pub type AgentIlm<D, L> =
    ILM<WrappedMessage, AgentBackend<D>, LocalDeliveryTx, AgentIlmTransport<L>>;

pub struct AgentIlmHost<D: LocalDbAccess, L: IlmPeerLinks> {
    cid: u64,
    ilm: AgentIlm<D, L>,
    inbound: UnboundedSender<InternalMessage>,
}

impl<D: LocalDbAccess, L: IlmPeerLinks> AgentIlmHost<D, L> {
    /// Loads the account's persisted ILM state and starts its loops.
    /// `delivered` receives each message once ILM delivers it, in order.
    pub async fn start(
        cid: u64,
        db: D,
        links: L,
        delivered: UnboundedSender<InternalServiceResponse>,
    ) -> Result<Self, BackendError<WrappedMessage>> {
        let backend = CitadelWorkspaceBackend::with_store(cid, AgentKvStore::new(cid, db));
        let (transport, inbound) = AgentIlmTransport::new(cid, links);
        let ilm = ILM::new(backend, LocalDeliveryTx::new(delivered), transport).await?;
        Ok(Self { cid, ilm, inbound })
    }

    pub fn cid(&self) -> u64 {
        self.cid
    }

    pub fn ilm(&self) -> &AgentIlm<D, L> {
        &self.ilm
    }

    /// Hand an arrived P2P message to this ILM.
    pub fn feed(&self, notification: MessageNotification) -> Result<(), FeedError> {
        let payload = match decode_inbound(notification) {
            Ok(InboundFrame::Signal(payload)) => payload,
            Ok(InboundFrame::Message { payload, .. }) => payload,
            Err((_not_a_frame, notification)) => return Err(FeedError::NotAFrame(notification)),
        };
        self.inbound.send(payload).map_err(|_| FeedError::Stopped)
    }
}

#[derive(Debug)]
pub enum FeedError {
    /// Raw P2P traffic (Yjs updates, plain messages), handed back untouched.
    NotAFrame(MessageNotification),
    /// The ILM's inbound loop has ended.
    Stopped,
}

enum Slot<D: LocalDbAccess, L: IlmPeerLinks> {
    /// Reserved by the start holding this ticket.
    Starting(u64),
    Running(Arc<AgentIlmHost<D, L>>),
}

#[derive(Debug)]
pub enum RegistryError {
    /// This CID already has an ILM, running or starting.
    AlreadyHosted(u64),
    /// Stopped while starting; the new ILM was discarded.
    StoppedWhileStarting(u64),
    /// Boxed: ILM's error carries a whole message inline.
    Backend(Box<BackendError<WrappedMessage>>),
}

pub struct AgentIlmRegistry<D: LocalDbAccess, L: IlmPeerLinks> {
    hosts: parking_lot::Mutex<HashMap<u64, Slot<D, L>>>,
    /// Distinguishes a start's reservation from a later one for the same CID
    /// (stop, then start again, while the first is still loading).
    tickets: AtomicU64,
}

impl<D: LocalDbAccess, L: IlmPeerLinks> Default for AgentIlmRegistry<D, L> {
    fn default() -> Self {
        Self {
            hosts: parking_lot::Mutex::new(HashMap::new()),
            tickets: AtomicU64::new(0),
        }
    }
}

impl<D: LocalDbAccess, L: IlmPeerLinks> AgentIlmRegistry<D, L> {
    /// Starts the ILM for `cid`. The slot is reserved before the (async) load,
    /// so two concurrent starts for one CID cannot both succeed.
    pub async fn start(
        &self,
        cid: u64,
        db: D,
        links: L,
        delivered: UnboundedSender<InternalServiceResponse>,
    ) -> Result<Arc<AgentIlmHost<D, L>>, RegistryError> {
        let ticket = self.tickets.fetch_add(1, Ordering::Relaxed);
        {
            let mut hosts = self.hosts.lock();
            if hosts.contains_key(&cid) {
                return Err(RegistryError::AlreadyHosted(cid));
            }
            hosts.insert(cid, Slot::Starting(ticket));
        }

        let started = AgentIlmHost::start(cid, db, links, delivered).await;

        let mut hosts = self.hosts.lock();
        let still_reserved = matches!(hosts.get(&cid), Some(Slot::Starting(t)) if *t == ticket);
        match started {
            Err(err) => {
                if still_reserved {
                    hosts.remove(&cid);
                }
                Err(RegistryError::Backend(Box::new(err)))
            }
            // `stop` ran meanwhile: honour it. Dropping the host stops its loops.
            Ok(_) if !still_reserved => Err(RegistryError::StoppedWhileStarting(cid)),
            Ok(host) => {
                let host = Arc::new(host);
                hosts.insert(cid, Slot::Running(host.clone()));
                Ok(host)
            }
        }
    }

    pub fn get(&self, cid: u64) -> Option<Arc<AgentIlmHost<D, L>>> {
        match self.hosts.lock().get(&cid) {
            Some(Slot::Running(host)) => Some(host.clone()),
            _ => None,
        }
    }

    /// Removes the CID's ILM; it stops once the last handle to it is dropped.
    /// A start still loading is cancelled: it finds its reservation gone and
    /// discards what it built.
    pub fn stop(&self, cid: u64) -> Option<Arc<AgentIlmHost<D, L>>> {
        match self.hosts.lock().remove(&cid) {
            Some(Slot::Running(host)) => Some(host),
            _ => None,
        }
    }
}
