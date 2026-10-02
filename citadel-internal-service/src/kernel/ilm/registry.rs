//! Which accounts have an agent-hosted ILM, and the one place one is started.

use super::channel::AgentChannel;
use super::delivery::AgentDelivery;
use super::transport::AgentTransport;
use super::HostIo;
use citadel_internal_service_connector::messenger::backend::CitadelWorkspaceBackend;
use citadel_internal_service_connector::messenger::{
    wire, CompressionHint, WrappedMessage, ACCOUNT_ILM_OPTIONS,
};
use citadel_internal_service_types::{
    InternalServicePayload, InternalServiceRequest, MessageNotification, SecurityLevel,
};
use citadel_sdk::logging::{error, info};
use intersession_layer_messaging::{BackendError, InboundFrame, ILM};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

pub(crate) type AgentIlm =
    ILM<WrappedMessage, CitadelWorkspaceBackend<AgentChannel>, AgentDelivery, AgentTransport>;

/// One account's ILM and the way frames are fed to it.
pub(crate) struct IlmHost {
    cid: u64,
    ilm: AgentIlm,
    inbound: UnboundedSender<InboundFrame<WrappedMessage>>,
}

impl IlmHost {
    /// The error is reduced to its text: ILM's own error type carries a whole
    /// message inline, and nothing here acts on more than what it says.
    async fn start(cid: u64, io: Arc<dyn HostIo>) -> Result<Self, String> {
        let backend = CitadelWorkspaceBackend::with_channel(cid, AgentChannel::new(io.clone()));
        let (transport, inbound) = AgentTransport::new(cid, io.clone());
        let delivery = AgentDelivery { cid, io };
        let ilm = ILM::new(backend, delivery, transport, ACCOUNT_ILM_OPTIONS)
            .await
            .map_err(|err: BackendError<WrappedMessage>| format!("{err:?}"))?;
        Ok(Self { cid, ilm, inbound })
    }

    /// Queue `message` for `peer_cid`. `Ok` means ILM has it and will keep
    /// sending it until the peer acknowledges -- not that it arrived.
    pub(crate) async fn send(
        &self,
        peer_cid: u64,
        request_id: Uuid,
        message: Vec<u8>,
        security_level: SecurityLevel,
        hint: Option<CompressionHint>,
    ) -> Result<(), String> {
        let payload = InternalServicePayload::Request(InternalServiceRequest::Message {
            request_id,
            message,
            cid: self.cid,
            peer_cid: Some(peer_cid),
            security_level,
        });
        self.ilm
            .send_to_with_hint(peer_cid, payload, hint)
            .await
            .map_err(|err| format!("{err:?}"))
    }
}

#[derive(Debug)]
pub(crate) enum StartError {
    /// Stopped (the session ended) while its stored state was loading.
    StoppedWhileStarting(u64),
    Backend(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::StoppedWhileStarting(cid) => {
                write!(f, "session {cid} ended while its ILM was loading")
            }
            StartError::Backend(reason) => {
                write!(f, "its stored state could not be read: {reason}")
            }
        }
    }
}

enum Slot {
    /// Reserved by the start holding this ticket.
    Starting(u64),
    Running(Arc<IlmHost>),
}

/// At most one ILM per CID, ever: the slot is reserved before the async load,
/// so two starts for one account cannot both succeed -- two ILMs over one
/// account's keys is exactly the corruption this module exists to prevent.
#[derive(Default)]
pub(crate) struct IlmRegistry {
    hosts: Mutex<HashMap<u64, Slot>>,
    tickets: AtomicU64,
}

impl IlmRegistry {
    /// The account's running ILM, starting it if there is none. A start
    /// already in progress for the CID is not duplicated: the caller is told
    /// to come back, as `None`.
    pub(crate) async fn ensure(
        &self,
        cid: u64,
        io: Arc<dyn HostIo>,
    ) -> Result<Option<Arc<IlmHost>>, StartError> {
        let ticket = self.tickets.fetch_add(1, Ordering::Relaxed);
        {
            let mut hosts = self.hosts.lock();
            match hosts.get(&cid) {
                Some(Slot::Running(host)) => return Ok(Some(host.clone())),
                Some(Slot::Starting(_)) => return Ok(None),
                None => {
                    hosts.insert(cid, Slot::Starting(ticket));
                }
            }
        }

        let started = IlmHost::start(cid, io).await;

        let mut hosts = self.hosts.lock();
        let still_reserved = matches!(hosts.get(&cid), Some(Slot::Starting(t)) if *t == ticket);
        match started {
            Err(err) => {
                if still_reserved {
                    hosts.remove(&cid);
                }
                error!(target: "citadel", "[ILM-HOST] {cid}: could not load its stored state: {err}");
                Err(StartError::Backend(err))
            }
            // Stopped meanwhile: honour it. Dropping the host stops its loops.
            Ok(_) if !still_reserved => Err(StartError::StoppedWhileStarting(cid)),
            Ok(host) => {
                let host = Arc::new(host);
                hosts.insert(cid, Slot::Running(host.clone()));
                info!(target: "citadel", "[ILM-HOST] {cid}: the agent now hosts this account's ILM");
                Ok(Some(host))
            }
        }
    }

    pub(crate) fn get(&self, cid: u64) -> Option<Arc<IlmHost>> {
        match self.hosts.lock().get(&cid) {
            Some(Slot::Running(host)) => Some(host.clone()),
            _ => None,
        }
    }

    /// Every account the agent hosts right now.
    pub(crate) fn hosted(&self) -> Vec<u64> {
        self.hosts
            .lock()
            .iter()
            .filter(|(_, slot)| matches!(slot, Slot::Running(_)))
            .map(|(cid, _)| *cid)
            .collect()
    }

    pub(crate) fn is_hosted(&self, cid: u64) -> bool {
        self.hosts.lock().contains_key(&cid)
    }

    /// The session ended. A start still loading finds its reservation gone and
    /// discards what it built.
    pub(crate) fn stop(&self, cid: u64) {
        if self.hosts.lock().remove(&cid).is_some() {
            info!(target: "citadel", "[ILM-HOST] {cid}: stopped");
        }
    }

    /// Hand an arrived P2P message to its account's ILM.
    ///
    /// `Err` gives the notification back, untouched, when it is not the ILM's:
    /// the account is not hosted, or the bytes are not an ILM frame (Yjs and the
    /// plain messaging service travel raw, by design). An ILM frame that cannot
    /// be read (unknown codec, corrupt) is dropped and logged, as the browser
    /// does: the sender retransmits it, and it is not a message anyone should see.
    pub(crate) fn feed(
        &self,
        notification: MessageNotification,
    ) -> Result<(), MessageNotification> {
        let Some(host) = self.get(notification.cid) else {
            return Err(notification);
        };
        match wire::decode_notification(&notification.message) {
            Ok(decoded) => {
                let frame = wire::into_inbound_frame(decoded, notification);
                if host.inbound.send(frame).is_err() {
                    error!(target: "citadel", "[ILM-HOST] {}: its ILM has stopped; frame dropped", host.cid);
                }
                Ok(())
            }
            Err(wire::DecodeError::Unreadable(reason)) => {
                error!(target: "ism", "[WIRE] dropping an extended frame from peer {}: {reason}", notification.peer_cid);
                Ok(())
            }
            Err(wire::DecodeError::NotAFrame(_)) => Err(notification),
        }
    }
}
