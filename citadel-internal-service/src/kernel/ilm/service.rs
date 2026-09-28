//! The agent's decisions about agent-hosted ILM, separate from its I/O
//! (docs/plans/multi-browser-cid.md, phase 2b).
//!
//! Whether it is offered, who is opted in, where an inbound P2P frame goes, and
//! what a reliable send does -- all over the registry in `host.rs`, generic in
//! the store, the peer links and the delivery target so the tests run the real
//! ILM without a node. The request handlers (`requests/agent_ilm.rs`) and the
//! two P2P read loops only supply the production implementations.
//!
//! Lifecycle: an ILM starts when its session opts in and stops when the session
//! is removed -- every removal path (Disconnect, Deregister, the SDK reporting
//! the session ended, a failed server reconnect, a stale entry replaced or
//! claimed away, DisconnectOrphan) calls `session_removed(cid)`
//! (kernel/cid_scoped_state.rs), which calls [`AgentIlmService::stop`]. A
//! dropped localhost connection removes no session (kernel/ext.rs), and a
//! server drop being reconnected keeps it, so neither stops anything: the ILM
//! keeps receiving, and holds deliveries until a subscriber is there.
use crate::kernel::ilm::host::{AgentIlmRegistry, Delivered, FeedError, RegistryError};
use crate::kernel::ilm::kv::LocalDbAccess;
use crate::kernel::ilm::transport::IlmPeerLinks;
use citadel_internal_service_connector::messenger::ilm::NetworkError;
use citadel_internal_service_connector::messenger::{DeliveryTarget, WrappedMessage};
use citadel_internal_service_types::{
    AgentIlmOffer, InternalServicePayload, InternalServiceRequest, MessageNotification,
    SecurityLevel,
};
use uuid::Uuid;

/// The agent's `multi_subscriber` setting. Explicit: nothing here assumes one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultiSubscriber {
    /// The agent never offers agent-hosted ILM; every session keeps the raw path.
    Off,
    On,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Enabled {
    Started,
    /// Idempotent: a reloaded page opts in again and finds its ILM running.
    AlreadyHosted,
}

#[derive(Debug)]
pub enum EnableError {
    NotOffered,
    /// Another opt-in for this account is still loading its state.
    StillStarting,
    /// The session was removed while its ILM was loading.
    StoppedWhileStarting,
    Backend(String),
}

#[derive(Debug)]
pub enum SendReliableError {
    /// Never falls back to the raw path: a browser that thinks it is opted in
    /// and is not must hear so, or its messages lose their reliability silently.
    NotOptedIn,
    Ilm(String),
}

/// Where one inbound P2P message went.
#[derive(Debug)]
pub enum Inbound {
    /// Not for an agent-hosted ILM: forward it to the browser exactly as before.
    Raw(MessageNotification),
    /// Fed to this session's agent-hosted ILM, which delivers it once in order.
    Hosted,
    /// The session's ILM has stopped taking input. Not forwarded raw: the
    /// browser is not running an ILM for this account and would mistake a frame
    /// for a message. Unacknowledged, so the peer's ILM sends it again.
    Lost,
}

pub struct AgentIlmService<D: LocalDbAccess, L: IlmPeerLinks, T: DeliveryTarget = Delivered> {
    setting: MultiSubscriber,
    registry: AgentIlmRegistry<D, L, T>,
}

impl<D: LocalDbAccess, L: IlmPeerLinks, T: DeliveryTarget> AgentIlmService<D, L, T> {
    pub fn new(setting: MultiSubscriber) -> Self {
        Self {
            setting,
            registry: AgentIlmRegistry::default(),
        }
    }

    pub fn setting(&self) -> MultiSubscriber {
        self.setting
    }

    /// What `GetSessionsResponse::agent_ilm` carries: absent unless offered.
    pub fn offer(&self) -> Option<AgentIlmOffer> {
        match self.setting {
            MultiSubscriber::Off => None,
            MultiSubscriber::On => Some(AgentIlmOffer {
                hosted: self.registry.hosted(),
            }),
        }
    }

    /// Opt `cid` in. The caller has already passed the ownership gate.
    ///
    /// PRECONDITION: the browser has stopped its own ILM for `cid`. Two ILMs
    /// for one account would each acknowledge and record the same frames and
    /// read-modify-write the same stored queues; the registry makes the agent
    /// run at most one, and only the browser can make sure it runs none.
    pub async fn enable(
        &self,
        cid: u64,
        db: D,
        links: L,
        delivered: T,
    ) -> Result<Enabled, EnableError> {
        if self.setting == MultiSubscriber::Off {
            return Err(EnableError::NotOffered);
        }
        if self.registry.get(cid).is_some() {
            return Ok(Enabled::AlreadyHosted);
        }
        match self.registry.start(cid, db, links, delivered).await {
            Ok(_host) => Ok(Enabled::Started),
            Err(RegistryError::AlreadyHosted(_)) if self.registry.get(cid).is_some() => {
                Ok(Enabled::AlreadyHosted)
            }
            Err(RegistryError::AlreadyHosted(_)) => Err(EnableError::StillStarting),
            Err(RegistryError::StoppedWhileStarting(_)) => Err(EnableError::StoppedWhileStarting),
            Err(RegistryError::Backend(err)) => Err(EnableError::Backend(describe_backend(*err))),
        }
    }

    pub fn is_hosted(&self, cid: u64) -> bool {
        self.registry.get(cid).is_some()
    }

    /// Queue `message` for `peer_cid` in `cid`'s agent-hosted ILM, framed as
    /// the browser's ILM frames a `Message` request.
    pub async fn send_reliable(
        &self,
        request_id: Uuid,
        cid: u64,
        peer_cid: u64,
        message: Vec<u8>,
        security_level: SecurityLevel,
    ) -> Result<(), SendReliableError> {
        let host = self
            .registry
            .get(cid)
            .ok_or(SendReliableError::NotOptedIn)?;
        let request = InternalServiceRequest::Message {
            request_id,
            message,
            cid,
            peer_cid: Some(peer_cid),
            security_level,
        };
        host.ilm()
            .send_to(peer_cid, InternalServicePayload::Request(request))
            .await
            .map_err(|err| SendReliableError::Ilm(describe(err)))
    }

    /// Route one message read from a P2P channel of session `notification.cid`.
    pub fn route_inbound(&self, notification: MessageNotification) -> Inbound {
        let Some(host) = self.registry.get(notification.cid) else {
            return Inbound::Raw(notification);
        };
        match host.feed(notification) {
            Ok(()) => Inbound::Hosted,
            // Yjs updates and other raw traffic are not ILM's, by design.
            Err(FeedError::NotAFrame(notification)) => Inbound::Raw(notification),
            Err(FeedError::Stopped) => Inbound::Lost,
        }
    }

    /// Stop `cid`'s ILM, if it has one. Its loops end once the last handle to
    /// it is dropped; a start still loading is cancelled.
    pub fn stop(&self, cid: u64) -> bool {
        self.registry.stop(cid).is_some()
    }
}

/// The reason, without the message: ILM's errors carry the undelivered
/// message inline, and this string goes into a response and the log.
fn describe(err: NetworkError<WrappedMessage>) -> String {
    match err {
        NetworkError::SendFailed { reason, .. } => format!("send failed: {reason}"),
        NetworkError::ConnectionError(reason) => format!("connection error: {reason}"),
        NetworkError::BackendError(err) => format!("storage error: {}", describe_backend(err)),
        NetworkError::ShutdownFailed(reason) => format!("shutdown failed: {reason}"),
        NetworkError::SystemShutdown => "the ILM has shut down".to_string(),
    }
}

fn describe_backend<M>(
    err: citadel_internal_service_connector::messenger::ilm::BackendError<M>,
) -> String {
    use citadel_internal_service_connector::messenger::ilm::BackendError;
    match err {
        BackendError::StorageError(reason) => reason,
        BackendError::SendFailed { reason, .. } => reason,
        BackendError::NotFound => "not found".to_string(),
    }
}
