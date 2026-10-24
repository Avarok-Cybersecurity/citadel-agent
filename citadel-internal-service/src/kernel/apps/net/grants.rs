//! What the server last said each install may reach, per session, and the questions still
//! waiting for an answer.
//!
//! An answer is reused for `GRANT_TTL_MS`, and dropped sooner when the server says the
//! member's installs changed, or when the session's link to the server goes down or comes
//! back. While the link is down nothing is answered from memory: the server could not have
//! told the agent about a change.

use super::authority::{AuthoritySignal, HostNetAuthority, KernelGrant};
use crate::kernel::server_link::SessionLink;
use citadel_app_net_policy::{GRANT_ANSWER_TIMEOUT_MS, GRANT_TTL_MS};
use citadel_sdk::logging::info;
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::Instant;

/// Why the server's word could not be had.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GrantFault {
    /// The session's link to the server is down.
    LinkDown,
    /// The question could not be encoded or sent.
    Send(String),
    /// No answer came within `GRANT_ANSWER_TIMEOUT_MS`.
    Timeout,
    /// The link went down, or the session ended, while waiting.
    Abandoned,
}

struct Pending {
    cid: u64,
    install_id: String,
    answer: oneshot::Sender<Option<KernelGrant>>,
}

#[derive(Default)]
struct State {
    entries: HashMap<(u64, String), (Option<KernelGrant>, Instant)>,
    pending: HashMap<u64, Pending>,
    down: HashSet<u64>,
    next_tag: u64,
}

impl State {
    /// Drops what is known about `cid`, and abandons its questions.
    fn forget(&mut self, cid: u64) {
        self.entries.retain(|(owner, _), _| *owner != cid);
        self.pending.retain(|_, pending| pending.cid != cid);
    }
}

pub(crate) struct GrantCache {
    authority: Arc<dyn HostNetAuthority>,
    state: Mutex<State>,
}

impl GrantCache {
    pub(crate) fn new(authority: Arc<dyn HostNetAuthority>) -> Self {
        Self {
            authority,
            state: Mutex::default(),
        }
    }

    /// What the server says `install_id` may reach for `cid`: from memory while fresh, or
    /// asked over `link` and awaited.
    pub(crate) async fn resolve(
        &self,
        cid: u64,
        install_id: &str,
        link: &dyn SessionLink,
    ) -> Result<Option<KernelGrant>, GrantFault> {
        let ttl = Duration::from_millis(GRANT_TTL_MS);
        let (tag, answer) = {
            let mut state = self.state.lock();
            if state.down.contains(&cid) {
                return Err(GrantFault::LinkDown);
            }
            let key = (cid, install_id.to_string());
            if let Some((grant, at)) = state.entries.get(&key) {
                if at.elapsed() < ttl {
                    return Ok(grant.clone());
                }
            }
            let tag = state.next_tag;
            state.next_tag = tag.wrapping_add(1);
            let (tx, rx) = oneshot::channel();
            state.pending.insert(
                tag,
                Pending {
                    cid,
                    install_id: key.1,
                    answer: tx,
                },
            );
            (tag, rx)
        };
        let sent = match self.authority.grant_query(install_id, tag) {
            Ok(frame) => link.send_to_server(cid, frame).await,
            Err(err) => Err(err),
        };
        if let Err(err) = sent {
            self.state.lock().pending.remove(&tag);
            return Err(GrantFault::Send(err));
        }
        let waited =
            tokio::time::timeout(Duration::from_millis(GRANT_ANSWER_TIMEOUT_MS), answer).await;
        match waited {
            Ok(Ok(grant)) => Ok(grant),
            Ok(Err(_)) => Err(GrantFault::Abandoned),
            Err(_) => {
                self.state.lock().pending.remove(&tag);
                Err(GrantFault::Timeout)
            }
        }
    }

    /// Reads what the server just sent `cid`; returns whether the frame goes on to the
    /// session's windows. The answer to the agent's own question stops here.
    pub(crate) fn on_server_frame(&self, cid: u64, frame: &[u8]) -> bool {
        match self.authority.classify(frame) {
            AuthoritySignal::Unrelated => true,
            AuthoritySignal::InstallsChanged => {
                self.state
                    .lock()
                    .entries
                    .retain(|(owner, _), _| *owner != cid);
                true
            }
            AuthoritySignal::Grant {
                install_id,
                tag,
                grant,
            } => {
                let mut state = self.state.lock();
                let ours = state
                    .pending
                    .get(&tag)
                    .is_some_and(|p| p.cid == cid && p.install_id == install_id);
                // Not a question the agent asked on this link: a window's, so it is theirs.
                let Some(pending) = ours.then(|| state.pending.remove(&tag)).flatten() else {
                    return true;
                };
                state
                    .entries
                    .insert((cid, install_id), (grant.clone(), Instant::now()));
                if pending.answer.send(grant).is_err() {
                    info!(target: "citadel", "[APP-NET] {cid}: a grant arrived after its fetch gave up");
                }
                false
            }
        }
    }

    /// The session's link to its server came up, new or reconnected.
    pub(crate) fn link_up(&self, cid: u64) {
        let mut state = self.state.lock();
        state.forget(cid);
        state.down.remove(&cid);
    }

    /// The session's link to its server dropped; the session lives on.
    pub(crate) fn link_down(&self, cid: u64) {
        let mut state = self.state.lock();
        state.forget(cid);
        state.down.insert(cid);
    }

    /// The session ended.
    pub(crate) fn session_ended(&self, cid: u64) {
        let mut state = self.state.lock();
        state.forget(cid);
        state.down.remove(&cid);
    }
}
