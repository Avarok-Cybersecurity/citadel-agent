//! `citadel.session` (CW-6, `citadel-session-abi.md` §4): an app's C2S session to an external
//! Citadel server, held by the member's agent under an account the agent owns.
//!
//! The account is a V2 identity account, `<id_fp>.app-<install_id[..8]>@<server>`, signed in
//! with the identity's derived password, and lives in the agent's ordinary session map: the
//! agent's own reconnect brings it back under the same CID when its server drops it.
//!
//! rules.rs decides (pure); this registry holds the sessions; core.rs runs a request against
//! the `SessionTransport` port; the requests (requests/app_session) supply the port.

pub(crate) mod core;
pub(crate) mod events;
pub(crate) mod pins;
pub(crate) mod rules;

use crate::kernel::apps::net::authority::{KernelGrant, SessionLimits};
use citadel_internal_service_types::{
    AppSessionMessage, AppSessionState, InternalServiceResponse, SessionStatus,
};
use events::SessionEvent;
use parking_lot::Mutex;
use rules::{Held, OpenDecision, Refusal};
use std::collections::HashMap;

pub use pins::{AppSessionConfig, PinEntry, PinError, PinnedServer};

/// Who a session is for: one per (install, acting user, server) on this member's session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Owner {
    pub member_cid: u64,
    pub install_id: String,
    pub acting_username: String,
}

#[derive(Debug, Clone)]
struct Entry {
    owner: Owner,
    server: String,
    status: SessionStatus,
    /// 0 until the account has signed in.
    server_cid: u64,
    limits: SessionLimits,
}

/// What an open found or made room for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reserved {
    Existing {
        session: u32,
        server_cid: u64,
        status: SessionStatus,
    },
    /// A new slot, `Connecting`, for `server` (canonical).
    New { session: u32, server: String },
}

#[derive(Default)]
struct State {
    entries: HashMap<u32, Entry>,
    last: u32,
}

#[derive(Default)]
pub(crate) struct AppSessions {
    state: Mutex<State>,
}

impl State {
    fn held(&self, owner: &Owner) -> Vec<Held> {
        let mine = self.entries.iter().filter(|(_, e)| &e.owner == owner);
        mine.map(|(id, e)| Held {
            server: e.server.clone(),
            session: *id,
            status: e.status.clone(),
        })
        .collect()
    }

    fn next_id(&mut self) -> u32 {
        loop {
            self.last = self.last.wrapping_add(1);
            if self.last != 0 && !self.entries.contains_key(&self.last) {
                return self.last;
            }
        }
    }
}

impl AppSessions {
    /// Decide the open under the lock and take the slot, so two opens for one server cannot
    /// both make a session.
    pub(crate) fn reserve(
        &self,
        grant: Option<&KernelGrant>,
        app_id: &str,
        owner: &Owner,
        server: &str,
    ) -> Result<Reserved, Refusal> {
        let mut state = self.state.lock();
        let held = state.held(owner);
        match rules::decide_open(grant, app_id, server, &held)? {
            OpenDecision::Reuse(session) => {
                let entry = &state.entries[&session];
                Ok(Reserved::Existing {
                    session,
                    server_cid: entry.server_cid,
                    status: entry.status.clone(),
                })
            }
            OpenDecision::Open {
                server,
                limits,
                replaces,
            } => {
                if let Some(closed) = replaces {
                    state.entries.remove(&closed);
                }
                let session = state.next_id();
                let entry = Entry {
                    owner: owner.clone(),
                    server: server.clone(),
                    status: SessionStatus::Connecting,
                    server_cid: 0,
                    limits,
                };
                state.entries.insert(session, entry);
                Ok(Reserved::New { session, server })
            }
        }
    }

    pub(crate) fn connected(&self, session: u32, server_cid: u64) {
        if let Some(entry) = self.state.lock().entries.get_mut(&session) {
            entry.server_cid = server_cid;
            entry.status = SessionStatus::Connected;
        }
    }

    /// Takes the session out; its server CID, if it ever signed in.
    pub(crate) fn remove(&self, session: u32) -> Option<u64> {
        let entry = self.state.lock().entries.remove(&session)?;
        Some(entry.server_cid)
    }

    /// The session, if it is this member's and this install's.
    fn owned<T>(
        &self,
        member_cid: u64,
        install_id: &str,
        session: u32,
        read: impl FnOnce(&Entry) -> T,
    ) -> Option<T> {
        let state = self.state.lock();
        let entry = state.entries.get(&session)?;
        let mine = entry.owner.member_cid == member_cid && entry.owner.install_id == install_id;
        mine.then(|| read(entry))
    }

    pub(crate) fn decide_send(
        &self,
        member_cid: u64,
        install_id: &str,
        session: u32,
        body_len: usize,
    ) -> Result<u64, Refusal> {
        let found = self.owned(member_cid, install_id, session, |e| {
            (e.status.clone(), e.server_cid, e.limits)
        });
        let view = found.as_ref().map(|(s, cid, l)| (s, *cid, *l));
        rules::decide_send(view, body_len)
    }

    pub(crate) fn is_owned(&self, member_cid: u64, install_id: &str, session: u32) -> bool {
        self.owned(member_cid, install_id, session, |_| ())
            .is_some()
    }

    /// The session's server CID (0 before it signs in); `None` once it is gone.
    pub(crate) fn server_cid_of(&self, session: u32) -> Option<u64> {
        let state = self.state.lock();
        state.entries.get(&session).map(|e| e.server_cid)
    }

    /// Whether `cid` is an account this registry holds a session for: not the member's own,
    /// so `GetSessions` does not list it.
    pub(crate) fn holds_server_cid(&self, cid: u64) -> bool {
        cid != 0
            && self
                .state
                .lock()
                .entries
                .values()
                .any(|e| e.server_cid == cid)
    }

    /// What `event` on `session`'s link tells its owner, after recording it. `None` once the
    /// session is gone: its link's last words reach nobody.
    pub(crate) fn on_event(
        &self,
        session: u32,
        event: SessionEvent,
    ) -> Option<(u64, InternalServiceResponse)> {
        let mut state = self.state.lock();
        let entry = state.entries.get_mut(&session)?;
        let (cid, install_id) = (entry.owner.member_cid, entry.owner.install_id.clone());
        let response = match event {
            SessionEvent::Message(body) => {
                InternalServiceResponse::AppSessionMessage(AppSessionMessage {
                    cid,
                    request_id: None,
                    install_id,
                    session,
                    body,
                })
            }
            SessionEvent::State(status) => {
                entry.status = status.clone();
                InternalServiceResponse::AppSessionState(AppSessionState {
                    cid,
                    request_id: None,
                    install_id,
                    session,
                    server_cid: entry.server_cid,
                    state: status,
                })
            }
        };
        Some((cid, response))
    }
}

#[cfg(test)]
#[path = "core_tests.rs"]
mod core_tests;
