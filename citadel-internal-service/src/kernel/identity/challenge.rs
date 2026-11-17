//! The post-connect `ServerIdentityChallenge` (`server-identity-challenge.md` §3): after every
//! successful connect the agent sends a fresh nonce over the session's own C2S link and checks
//! the answer against the account's pin. The judgement is pure (`judge`); the frames are the
//! workspace protocol's, encoded by a codec the binary supplies, as host egress's authority is
//! (apps/net/authority.rs); the wait for the answer is `ChallengeWaiters`.

use super::pin::Pin;
use citadel_identity::{fingerprint, verify_server_challenge, MlDsa65Pk, NONCE_LEN};
use parking_lot::Mutex;
use std::collections::HashMap;
use tokio::sync::oneshot;

/// `WorkspaceProtocolResponse::ServerIdentity`, decoded.
#[derive(Clone, PartialEq, Eq)]
pub struct ServerIdentityAnswer {
    pub domain: String,
    pub pk: Vec<u8>,
    pub sig: Vec<u8>,
}

impl std::fmt::Debug for ServerIdentityAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ServerIdentityAnswer {{ domain: {:?}, pk: <{} bytes>, sig: <{} bytes> }}",
            self.domain,
            self.pk.len(),
            self.sig.len()
        )
    }
}

/// The workspace protocol's encoding of the challenge and its answer.
pub trait ServerIdentityCodec: Send + Sync + 'static {
    fn challenge(&self, nonce: &[u8; NONCE_LEN]) -> Result<Vec<u8>, String>;
    /// `Some` when `frame` is a `ServerIdentity` answer; every other frame is the windows'.
    fn answer(&self, frame: &[u8]) -> Option<ServerIdentityAnswer>;
}

/// Why the server's answer is not the pinned server's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Mismatch {
    pub(crate) pinned_fp: String,
    /// The answering key's fingerprint; empty when the answer carried no usable key.
    pub(crate) offered_fp: String,
    pub(crate) why: String,
}

/// Accepts only the pinned key's (or, before it is known, the pinned fingerprint's key's)
/// signature over this nonce, for this session's CID, under the pinned domain. Returns the
/// key, so a `Pinned` registration can record it the first time it is proved.
pub(crate) fn judge(
    pin: &Pin,
    cid: u64,
    nonce: &[u8; NONCE_LEN],
    answer: &ServerIdentityAnswer,
) -> Result<MlDsa65Pk, Mismatch> {
    let mismatch = |offered_fp: String, why: String| Mismatch {
        pinned_fp: pin.server_fp.clone(),
        offered_fp,
        why,
    };
    let offered = MlDsa65Pk::from_bytes(&answer.pk)
        .map_err(|err| mismatch(String::new(), err.to_string()))?;
    let offered_fp = fingerprint(&offered);
    let key_is_pinned = match &pin.server_pk {
        Some(pinned) => *pinned == offered,
        None => offered_fp == pin.server_fp,
    };
    if !key_is_pinned {
        return Err(mismatch(
            offered_fp,
            "the server answered with another key".into(),
        ));
    }
    if answer.domain != pin.domain.as_str() {
        let why = format!(
            "the server calls itself {:?}, not {}",
            answer.domain, pin.domain
        );
        return Err(mismatch(offered_fp, why));
    }
    verify_server_challenge(&offered, &pin.domain, cid, nonce, &answer.sig)
        .map_err(|err| mismatch(offered_fp, err.to_string()))?;
    Ok(offered)
}

/// The challenges the agent is waiting on, one per session.
pub(crate) struct ChallengeWaiters {
    codec: std::sync::Arc<dyn ServerIdentityCodec>,
    pending: Mutex<HashMap<u64, oneshot::Sender<ServerIdentityAnswer>>>,
}

impl ChallengeWaiters {
    pub(crate) fn new(codec: std::sync::Arc<dyn ServerIdentityCodec>) -> Self {
        Self {
            codec,
            pending: Mutex::default(),
        }
    }

    pub(crate) fn codec(&self) -> &dyn ServerIdentityCodec {
        &*self.codec
    }

    /// Waits for session `cid`'s next answer; a newer wait replaces an older one.
    pub(crate) fn await_answer(&self, cid: u64) -> oneshot::Receiver<ServerIdentityAnswer> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(cid, tx);
        rx
    }

    pub(crate) fn forget(&self, cid: u64) {
        self.pending.lock().remove(&cid);
    }

    /// Reads what the server just sent `cid`; returns whether the frame goes on to the
    /// session's windows. An answer the agent is waiting for stops here; one it is not (a
    /// window asked) goes on.
    pub(crate) fn on_server_frame(&self, cid: u64, frame: &[u8]) -> bool {
        let Some(answer) = self.codec.answer(frame) else {
            return true;
        };
        match self.pending.lock().remove(&cid) {
            Some(waiter) => {
                let _gone = waiter.send(answer);
                false
            }
            None => true,
        }
    }
}

#[cfg(test)]
#[path = "challenge_tests.rs"]
mod tests;
