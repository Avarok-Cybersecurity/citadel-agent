//! Relaying the SDK's security-key requests to a window and its answer back.
//!
//! The SDK never touches an authenticator: when a sign-in or a management change needs a key, it
//! sends a `SecurityKeyChallenge` down a channel and waits at most `KEY_PRESENCE_WINDOW`. Only a
//! browser can run WebAuthn, so the agent forwards each request as a
//! `SecurityKeyChallengeNotification` to the windows that may answer it, and passes the first
//! valid `SecurityKeyAnswer` on.
//!
//! What "valid" means is checked here, before the challenge is spent, so a bad answer cannot use
//! up the touch a good one needs: the challenge must still be open, the answer must come
//! from a window it was sent to, name an allowed credential, and carry a 32-byte PRF output.
//!
//! The agent does not wait for anyone: the SDK's own deadline bounds the wait. A relay lives as
//! long as the request that asked, and that request ends when the SDK gives up on the touch, so
//! its open challenges close with it and a late answer finds nothing to answer.

use crate::kernel::session_route::{deliver, Clients};
use citadel_internal_service_types::{
    InternalServiceResponse, SecurityKeyChallengeNotification, SecurityKeyPurpose,
};
use citadel_proto::auth::{SecurityKeyPurpose as SdkPurpose, KEY_PRESENCE_WINDOW};
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::{security_key_channel, SecBuffer, SecurityKeyChallenge, SecurityKeyPrf};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::JoinHandle;
use uuid::Uuid;
use zeroize::Zeroizing;

const PRF_LEN: usize = 32;

/// Every open challenge on this agent, by challenge id.
#[derive(Default)]
pub(crate) struct KeyChallenges(Mutex<HashMap<Uuid, Open>>);

struct Open {
    relay: Uuid,
    cid: u64,
    audience: Vec<Uuid>,
    allowed: Vec<Vec<u8>>,
    challenge: SecurityKeyChallenge,
}

/// Why an answer was not used. The challenge stays open for a valid one.
pub(crate) struct Refused {
    pub cid: u64,
    pub message: String,
}

/// Who a relay's challenges are for, and what their notifications carry.
pub(crate) struct Asker {
    /// The windows that may answer: the one that is signing in, or a session's windows.
    pub audience: Vec<Uuid>,
    /// The session's CID, or 0 for a sign-in, which has no session yet.
    pub cid: u64,
    /// The asking request; every notification carries it.
    pub request_id: Uuid,
}

/// The agent's end of one request's key channel. Dropping it closes its open challenges.
pub(crate) struct KeyRelay {
    id: Uuid,
    asked: Arc<AtomicBool>,
    task: JoinHandle<()>,
    challenges: Arc<KeyChallenges>,
}

impl KeyRelay {
    /// A key channel for the SDK, served for `asker` until the relay is dropped.
    pub(crate) fn start(
        challenges: &Arc<KeyChallenges>,
        clients: &Clients,
        asker: Asker,
    ) -> (SecurityKeyPrf, Self) {
        let (key, touches) = security_key_channel();
        let id = Uuid::new_v4();
        let asked = Arc::new(AtomicBool::new(false));
        let task = tokio::spawn(serve(
            challenges.clone(),
            clients.clone(),
            asker,
            id,
            asked.clone(),
            touches,
        ));
        let relay = Self {
            id,
            asked,
            task,
            challenges: challenges.clone(),
        };
        (key, relay)
    }

    /// Whether the SDK asked for a touch: the account's policy required a key.
    pub(crate) fn asked(&self) -> bool {
        self.asked.load(Ordering::SeqCst)
    }
}

impl Drop for KeyRelay {
    fn drop(&mut self) {
        self.task.abort();
        self.challenges
            .0
            .lock()
            .retain(|_, open| open.relay != self.id);
    }
}

async fn serve(
    challenges: Arc<KeyChallenges>,
    clients: Clients,
    asker: Asker,
    relay: Uuid,
    asked: Arc<AtomicBool>,
    mut touches: UnboundedReceiver<SecurityKeyChallenge>,
) {
    while let Some(challenge) = touches.recv().await {
        asked.store(true, Ordering::SeqCst);
        let challenge_id = Uuid::new_v4();
        let notification = SecurityKeyChallengeNotification {
            cid: asker.cid,
            request_id: Some(asker.request_id),
            challenge_id,
            purpose: purpose(challenge.purpose),
            allowed_credential_ids: challenge.request.credential_ids.clone(),
            prf_salt: challenge.request.prf_eval_salt.to_vec(),
            expires_in_ms: KEY_PRESENCE_WINDOW.as_millis() as u64,
        };
        challenges.0.lock().insert(
            challenge_id,
            Open {
                relay,
                cid: asker.cid,
                audience: asker.audience.clone(),
                allowed: challenge.request.credential_ids.clone(),
                challenge,
            },
        );
        let response = InternalServiceResponse::SecurityKeyChallengeNotification(notification);
        let reached = deliver(&clients, &asker.audience, response);
        if reached.is_empty() {
            // Nobody to touch the key: the SDK times out at its deadline and says so.
            warn!(target: "citadel", "[SignIn] No window received key challenge {challenge_id} for request {}", asker.request_id);
        } else {
            info!(target: "citadel", "[SignIn] Key challenge {challenge_id} sent to {} window(s)", reached.len());
        }
    }
}

fn purpose(purpose: SdkPurpose) -> SecurityKeyPurpose {
    match purpose {
        SdkPurpose::SignIn => SecurityKeyPurpose::SignIn,
        SdkPurpose::StepUp => SecurityKeyPurpose::StepUp,
        SdkPurpose::Enrol => SecurityKeyPurpose::Enrol,
    }
}

impl KeyChallenges {
    /// Hands `prf_output` for `credential_id` to the SDK if the answer is valid; the session's
    /// CID either way.
    pub(crate) fn answer(
        &self,
        from: Uuid,
        challenge_id: Uuid,
        credential_id: Vec<u8>,
        prf_output: &SecBuffer,
    ) -> Result<u64, Refused> {
        let open = self.take(from, challenge_id, |open| {
            if !open.allowed.contains(&credential_id) {
                return Err("The key that answered is not one this challenge allows".into());
            }
            if prf_output.as_ref().len() != PRF_LEN {
                return Err(format!("A PRF output is {PRF_LEN} bytes"));
            }
            Ok(())
        })?;
        let mut prf = Zeroizing::new([0u8; PRF_LEN]);
        prf.copy_from_slice(prf_output.as_ref());
        open.challenge.answer(credential_id, *prf);
        Ok(open.cid)
    }

    /// Fails the asking request now rather than at the deadline.
    pub(crate) fn decline(
        &self,
        from: Uuid,
        challenge_id: Uuid,
        reason: String,
    ) -> Result<u64, Refused> {
        let open = self.take(from, challenge_id, |_| Ok(()))?;
        open.challenge.decline(reason);
        Ok(open.cid)
    }

    /// Removes the challenge if it is open to `from` and `check` accepts the answer; otherwise
    /// leaves it open.
    fn take(
        &self,
        from: Uuid,
        challenge_id: Uuid,
        check: impl FnOnce(&Open) -> Result<(), String>,
    ) -> Result<Open, Refused> {
        let mut map = self.0.lock();
        let verdict = match map.get(&challenge_id) {
            None => Err(Refused {
                cid: 0,
                message: "No such key challenge is open: it was answered, it expired, or the \
                          request that raised it has ended"
                    .into(),
            }),
            Some(open) => judge(open, from, check).map_err(|message| Refused {
                cid: open.cid,
                message,
            }),
        };
        verdict?;
        map.remove(&challenge_id).ok_or(Refused {
            cid: 0,
            message: "The key challenge closed while it was being answered".into(),
        })
    }
}

fn judge(
    open: &Open,
    from: Uuid,
    check: impl FnOnce(&Open) -> Result<(), String>,
) -> Result<(), String> {
    if !open.audience.contains(&from) {
        return Err("This window was not asked to answer that challenge".into());
    }
    check(open)
}
