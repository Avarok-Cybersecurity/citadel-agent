//! The collector's pure core: which sessions exist, which opted in, what each ring holds.
//!
//! No clock, no randomness, no locking: the shell supplies the time on each event and a
//! fresh salt on each opt-in and drain.

use super::config::{RingConfig, TelemetryError};
use super::entry::{CapturedEvent, TelemetryEntry};
use super::redact::{cid_tokens, redact, KnownValue, Salt};
use std::collections::{HashMap, VecDeque};

#[derive(Debug)]
pub struct CollectorCore {
    config: RingConfig,
    sessions: HashMap<u64, LocalSession>,
    /// Derived from `sessions` on every change to identities; never edited directly.
    known: Vec<KnownValue>,
}

#[derive(Debug, Default)]
struct LocalSession {
    identity: Vec<KnownValue>,
    peers: HashMap<u64, Vec<KnownValue>>,
    ring: Option<Ring>,
}

#[derive(Debug)]
struct Ring {
    entries: VecDeque<TelemetryEntry>,
    bytes: usize,
    salt: Salt,
}

impl CollectorCore {
    pub fn new(config: RingConfig) -> Self {
        Self {
            config,
            sessions: HashMap::new(),
            known: Vec::new(),
        }
    }

    /// Records a local session and the values that identify its account. Re-registering
    /// replaces the identity and keeps the session's opt-in and ring.
    pub fn register_session(&mut self, cid: u64, identity: Vec<KnownValue>) {
        self.sessions.entry(cid).or_default().identity = identity;
        self.rebuild_known();
    }

    /// The session ended: its ring, its opt-in and its identities go with it.
    pub fn forget_session(&mut self, cid: u64) {
        if self.sessions.remove(&cid).is_some() {
            self.rebuild_known();
        }
    }

    pub fn learn_peer(
        &mut self,
        session_cid: u64,
        peer_cid: u64,
        identity: Vec<KnownValue>,
    ) -> Result<(), TelemetryError> {
        let session = self
            .sessions
            .get_mut(&session_cid)
            .ok_or(TelemetryError::UnknownSession(session_cid))?;
        session.peers.insert(peer_cid, identity);
        self.rebuild_known();
        Ok(())
    }

    /// Starts capturing for `cid`. Already opted in: the ring and its salt are kept.
    pub fn opt_in(&mut self, cid: u64, salt: Salt) -> Result<(), TelemetryError> {
        let session = self.session_mut(cid)?;
        session.ring.get_or_insert_with(|| Ring {
            entries: VecDeque::new(),
            bytes: 0,
            salt,
        });
        Ok(())
    }

    /// Stops capturing for `cid` and discards everything its ring held.
    pub fn opt_out(&mut self, cid: u64) -> Result<(), TelemetryError> {
        self.session_mut(cid)?.ring = None;
        Ok(())
    }

    pub fn is_capturing(&self) -> bool {
        self.sessions.values().any(|session| session.ring.is_some())
    }

    /// Takes everything `cid` holds and starts a new batch under `next_salt`.
    pub fn drain(
        &mut self,
        cid: u64,
        next_salt: Salt,
    ) -> Result<Vec<TelemetryEntry>, TelemetryError> {
        let ring = self
            .session_mut(cid)?
            .ring
            .as_mut()
            .ok_or(TelemetryError::NotOptedIn(cid))?;
        ring.bytes = 0;
        ring.salt = next_salt;
        Ok(ring.entries.drain(..).collect())
    }

    /// A session's own event goes to that session only. An agent-wide event goes to every
    /// opted-in session, except one that names a DIFFERENT local session's cid: that line
    /// describes another account on this machine. Remote peer cids are not local sessions
    /// and do not drop a line.
    pub fn capture(&mut self, event: &CapturedEvent<'_>) {
        let config = self.config;
        let known = &self.known;
        match event.session_cid {
            Some(cid) => {
                if let Some(ring) = self.sessions.get_mut(&cid).and_then(|s| s.ring.as_mut()) {
                    ring.push(entry(event, known, &ring.salt, &config), &config);
                }
            }
            None => {
                let named_local: Vec<u64> = cid_tokens(event.message)
                    .filter(|named| self.sessions.contains_key(named))
                    .collect();
                for (cid, session) in &mut self.sessions {
                    let Some(ring) = session.ring.as_mut() else {
                        continue;
                    };
                    if named_local.iter().any(|named| named != cid) {
                        continue;
                    }
                    ring.push(entry(event, known, &ring.salt, &config), &config);
                }
            }
        }
    }

    fn session_mut(&mut self, cid: u64) -> Result<&mut LocalSession, TelemetryError> {
        self.sessions
            .get_mut(&cid)
            .ok_or(TelemetryError::UnknownSession(cid))
    }

    /// Every identity any local session knows is redacted in every session's lines: a
    /// name that one account knows can appear in an agent-wide line delivered to another.
    fn rebuild_known(&mut self) {
        self.known = self
            .sessions
            .values()
            .flat_map(|s| s.identity.iter().chain(s.peers.values().flatten()))
            .cloned()
            .collect();
    }
}

fn entry(
    event: &CapturedEvent<'_>,
    known: &[KnownValue],
    salt: &Salt,
    config: &RingConfig,
) -> TelemetryEntry {
    let mut message = redact(event.message, known, salt);
    truncate_at_char(&mut message, config.max_message_bytes());
    let mut target = event.target.to_owned();
    truncate_at_char(&mut target, config.max_message_bytes());
    TelemetryEntry {
        time_ms: event.time_ms,
        level: event.level,
        target,
        message,
        peer_cid: event.peer_cid,
    }
}

fn truncate_at_char(text: &mut String, max: usize) {
    if text.len() > max {
        let cut = (0..=max)
            .rev()
            .find(|i| text.is_char_boundary(*i))
            .unwrap_or(0);
        text.truncate(cut);
    }
}

impl Ring {
    /// Evicts oldest-first until the new entry fits both bounds.
    fn push(&mut self, entry: TelemetryEntry, config: &RingConfig) {
        let size = entry.byte_len();
        while !self.entries.is_empty()
            && (self.entries.len() >= config.max_entries()
                || self.bytes + size > config.max_bytes())
        {
            if let Some(old) = self.entries.pop_front() {
                self.bytes -= old.byte_len();
            }
        }
        self.bytes += size;
        self.entries.push_back(entry);
    }
}

#[cfg(test)]
mod tests;
