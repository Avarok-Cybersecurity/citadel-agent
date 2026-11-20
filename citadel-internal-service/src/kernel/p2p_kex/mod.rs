//! The node's P2P key-exchange identity (the SDK's `P2pKexIdentity`): each session signs its
//! stage of every P2P key exchange with its identity key, and checks the peer's stage against the
//! key it pinned for that peer.
//!
//! A session opened with the identity holds a signer for its whole life (`Connection`'s
//! `p2p_kex_signer`, set by `ConnectIdentity`); a legacy account's session holds none, and its
//! stages go unsigned. The pins are this agent's (`pins.rs`), per account and peer. A peer whose
//! key changed is refused until the person accepts the new key: the windows of the session hear
//! `PeerIdentityKeyChangedNotification`, and only `AcceptPeerIdentityKey` re-pins.

mod book;
pub(crate) mod pins;

pub(crate) use book::grouped_fingerprint_of;
pub use pins::{peer_key_store_for, FsPeerKeyStore, MemoryPeerKeyStore, PeerKeyStore};

use crate::kernel::session_route::{Clients, SessionRoute};
use crate::kernel::Connection;
use citadel_internal_service_types::{
    InternalServiceResponse, PeerIdentityKeyChanged, PeerIdentityMark,
};
use citadel_sdk::prelude::{KexAuth, P2pKexIdentity, PeerKexIdentity, Ratchet};
use futures::future::BoxFuture;
use parking_lot::RwLock;
use pins::PeerKeyPins;
use std::collections::HashMap;
use std::sync::Arc;

/// The port the node is built with.
pub(crate) struct KexIdentityPort<R: Ratchet> {
    pub(crate) sessions: Arc<RwLock<HashMap<u64, Connection<R>>>>,
    pub(crate) clients: Clients,
    pub(crate) pins: Arc<PeerKeyPins>,
}

impl<R: Ratchet> P2pKexIdentity for KexIdentityPort<R> {
    fn sign(&self, local_cid: u64, body: &[u8]) -> Option<KexAuth> {
        let signer = self
            .sessions
            .read()
            .get(&local_cid)?
            .p2p_kex_signer
            .clone()?;
        Some(KexAuth {
            identity_key: signer.public_key().as_bytes().to_vec(),
            signature: signer.sign(body),
        })
    }

    fn pinned_key(&self, local_cid: u64, peer_cid: u64) -> Option<Vec<u8>> {
        self.pins.pinned(local_cid, peer_cid)
    }

    fn pin_first_key(
        &self,
        local_cid: u64,
        peer_cid: u64,
        identity_key: &[u8],
    ) -> BoxFuture<'static, Result<(), String>> {
        let (pins, key) = (self.pins.clone(), identity_key.to_vec());
        Box::pin(async move { pins.pin_first(local_cid, peer_cid, &key).await })
    }

    fn peer_key_changed(&self, local_cid: u64, peer_cid: u64, presented: &[u8]) {
        if !self.pins.record_change(local_cid, peer_cid, presented) {
            return;
        }
        let pinned = self.pins.pinned(local_cid, peer_cid);
        let notification =
            InternalServiceResponse::PeerIdentityKeyChangedNotification(PeerIdentityKeyChanged {
                cid: local_cid,
                peer_cid,
                pinned_fingerprint: pinned
                    .as_deref()
                    .and_then(grouped_fingerprint_of)
                    .unwrap_or_default(),
                presented_fingerprint: grouped_fingerprint_of(presented).unwrap_or_default(),
                request_id: None,
            });
        let subscribers = self
            .sessions
            .read()
            .get(&local_cid)
            .map(|session| session.subscribers.clone());
        let reached = subscribers
            .map(|subs| SessionRoute::new(subs, self.clients.clone()).send(notification))
            .unwrap_or_default();
        citadel_sdk::logging::warn!(target: "citadel", "[P2pKex] {local_cid}: peer {peer_cid} signed with a new identity key; refused until accepted (told {} window(s))", reached.len());
    }
}

/// What a channel's key exchange proved, as the windows show it.
pub(crate) fn mark(identity: Option<&PeerKexIdentity>) -> PeerIdentityMark {
    match identity {
        Some(PeerKexIdentity::Signed { identity_key }) => {
            match grouped_fingerprint_of(identity_key) {
                Some(fingerprint) => PeerIdentityMark::Verified { fingerprint },
                None => PeerIdentityMark::Unverified,
            }
        }
        Some(PeerKexIdentity::Unsigned) | None => PeerIdentityMark::Unverified,
    }
}

#[cfg(test)]
#[path = "port_tests.rs"]
mod port_tests;
