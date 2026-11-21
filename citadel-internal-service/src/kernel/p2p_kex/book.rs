//! The identity key each account pinned for each peer, and a changed key waiting for the
//! person's acceptance. Pure: no I/O; `pins.rs` persists it.
//!
//! A pin is made the first time a peer signs an exchange with a verified key, and changes only
//! when the person accepts the key a refused exchange presented, naming it by its fingerprint.

use citadel_identity::{
    base64url_decode, base64url_encode, fingerprint, fingerprint_grouped, MlDsa65Pk,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PeerPin {
    pub(crate) key: Vec<u8>,
    /// A key the peer signed with since, refused until the person accepts it.
    pub(crate) pending: Option<Vec<u8>>,
}

#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub(crate) struct PinBook {
    pins: BTreeMap<(u64, u64), PeerPin>,
}

/// Why an accept pinned nothing.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NotAccepted {
    NothingPending,
    /// The pending key is not the one the person was shown.
    OtherKey {
        pending: String,
    },
}

impl std::fmt::Display for NotAccepted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NothingPending => write!(f, "no changed key is waiting for this peer"),
            Self::OtherKey { pending } => {
                write!(f, "the waiting key is {pending}, not the one accepted")
            }
        }
    }
}

/// The 16-character fingerprint of an encoded identity key, `None` if it is not one.
pub(crate) fn fingerprint_of(key: &[u8]) -> Option<String> {
    MlDsa65Pk::from_bytes(key).ok().map(|pk| fingerprint(&pk))
}

/// `abcd-efgh-ijkl-mnop`, as the person compares it.
pub(crate) fn grouped_fingerprint_of(key: &[u8]) -> Option<String> {
    MlDsa65Pk::from_bytes(key)
        .ok()
        .map(|pk| fingerprint_grouped(&pk))
}

impl PinBook {
    pub(crate) fn pin(&self, local: u64, peer: u64) -> Option<&PeerPin> {
        self.pins.get(&(local, peer))
    }

    /// Pins `key` for a peer with no pin; `false` if one was already there.
    pub(crate) fn pin_first(&mut self, local: u64, peer: u64, key: &[u8]) -> bool {
        if self.pins.contains_key(&(local, peer)) {
            return false;
        }
        let pin = PeerPin {
            key: key.to_vec(),
            pending: None,
        };
        self.pins.insert((local, peer), pin);
        true
    }

    /// Records the key a refused exchange presented; `true` if it is newly waiting.
    pub(crate) fn record_change(&mut self, local: u64, peer: u64, presented: &[u8]) -> bool {
        match self.pins.get_mut(&(local, peer)) {
            Some(pin) if pin.pending.as_deref() != Some(presented) => {
                pin.pending = Some(presented.to_vec());
                true
            }
            _ => false,
        }
    }

    /// Pins the waiting key, if it is the one whose fingerprint the person accepted.
    pub(crate) fn accept(
        &mut self,
        local: u64,
        peer: u64,
        accepted_fingerprint: &str,
    ) -> Result<Vec<u8>, NotAccepted> {
        let pin = self.pins.get_mut(&(local, peer));
        let pending = pin
            .as_ref()
            .and_then(|pin| pin.pending.clone())
            .ok_or(NotAccepted::NothingPending)?;
        let accepted = accepted_fingerprint.replace('-', "");
        if fingerprint_of(&pending).as_deref() != Some(accepted.as_str()) {
            return Err(NotAccepted::OtherKey {
                pending: grouped_fingerprint_of(&pending).unwrap_or_default(),
            });
        }
        let pin = pin.ok_or(NotAccepted::NothingPending)?;
        pin.key = pending.clone();
        pin.pending = None;
        Ok(pending)
    }
}

#[derive(Serialize, Deserialize)]
struct StoredPin {
    local_cid: u64,
    peer_cid: u64,
    key: String,
    pending: Option<String>,
}

impl PinBook {
    pub(crate) fn encode(&self) -> Vec<u8> {
        let stored: Vec<StoredPin> = self
            .pins
            .iter()
            .map(|(&(local_cid, peer_cid), pin)| StoredPin {
                local_cid,
                peer_cid,
                key: base64url_encode(&pin.key),
                pending: pin.pending.as_deref().map(base64url_encode),
            })
            .collect();
        serde_json::to_vec(&stored).unwrap_or_else(|_| unreachable!("pins always encode"))
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, String> {
        let stored: Vec<StoredPin> = serde_json::from_slice(bytes)
            .map_err(|err| format!("the stored peer identity keys do not parse: {err}"))?;
        let mut book = PinBook::default();
        for entry in stored {
            let key = base64url_decode(&entry.key)?;
            let pending = entry.pending.as_deref().map(base64url_decode).transpose()?;
            book.pins
                .insert((entry.local_cid, entry.peer_cid), PeerPin { key, pending });
        }
        Ok(book)
    }
}

#[cfg(test)]
#[path = "book_tests.rs"]
mod tests;
