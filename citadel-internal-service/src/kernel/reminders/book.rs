//! One session's reminders and what became of them, decided without a clock or a store: the
//! runner (runner.rs) reads the time, the store and the windows, and asks this.
//!
//! - `sync` replaces one app's reminders with what the page holds now. A reminder already fired
//!   is never taken back in, whatever the page sends: that is what keeps a resync, a second
//!   window or a restart from firing it twice.
//! - `take_due` moves what is due into `fired` before anything is shown (at most once).
//! - What fired while nothing could show it is held, to be shown once, late.

use citadel_internal_service_types::{AppNotice, AppNoticeRefusal};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// The most reminders one session holds, across its apps.
pub(crate) const CAP: usize = 512;
/// Fired this long after its time, a reminder is late.
pub(crate) const LATE_AFTER_MS: u64 = 60_000;
const ID_MAX: usize = 160;
const APP_ID_MAX: usize = 64;
const NODE_ID_MAX: usize = 128;
const TITLE_MAX: usize = 200;
const BODY_MAX: usize = 1_000;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fired {
    pub app_id: String,
    pub id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub(crate) struct Book {
    pending: Vec<AppNotice>,
    fired: Vec<Fired>,
    held: Vec<AppNotice>,
}

fn all_of(s: &str, max: usize, allowed: impl Fn(char) -> bool) -> bool {
    !s.is_empty() && s.len() <= max && s.chars().all(allowed)
}

/// Whether the agent will show `notice`: every field it names is bounded, and the node is an id,
/// so what a click opens (`node:<node_id>`) can only ever be a node of the workspace.
pub(crate) fn acceptable(notice: &AppNotice) -> bool {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    all_of(&notice.id, ID_MAX, |c| word(c) || c == ':' || c == '.')
        && all_of(&notice.app_id, APP_ID_MAX, |c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-')
        })
        && all_of(&notice.node_id, NODE_ID_MAX, word)
        && !notice.title.is_empty()
        && notice.title.chars().count() <= TITLE_MAX
        && !notice.title.chars().any(char::is_control)
        && notice.body.chars().count() <= BODY_MAX
        && !notice.body.chars().any(|c| c.is_control() && c != '\n')
}

impl Book {
    pub(crate) fn empty() -> Self {
        Self {
            pending: Vec::new(),
            fired: Vec::new(),
            held: Vec::new(),
        }
    }

    /// The book with `app_id`'s reminders replaced by `incoming`; nothing changes on a refusal.
    pub(crate) fn sync(
        &self,
        app_id: &str,
        incoming: Vec<AppNotice>,
    ) -> Result<Book, AppNoticeRefusal> {
        let fits = |n: &AppNotice| acceptable(n) && n.app_id == app_id && n.fire_at_ms.is_some();
        if !incoming.iter().all(fits) {
            return Err(AppNoticeRefusal::BadTarget);
        }
        let incoming_ids: HashSet<&str> = incoming.iter().map(|n| n.id.as_str()).collect();
        let fired: Vec<Fired> = self
            .fired
            .iter()
            .filter(|f| f.app_id != app_id || incoming_ids.contains(f.id.as_str()))
            .cloned()
            .collect();
        let already: HashSet<&str> = fired
            .iter()
            .filter(|f| f.app_id == app_id)
            .map(|f| f.id.as_str())
            .collect();
        let mut seen: HashSet<String> = HashSet::new();
        let mut pending: Vec<AppNotice> = self
            .pending
            .iter()
            .filter(|n| n.app_id != app_id)
            .cloned()
            .collect();
        for notice in incoming {
            if !already.contains(notice.id.as_str()) && seen.insert(notice.id.clone()) {
                pending.push(notice);
            }
        }
        if pending.len() > CAP {
            return Err(AppNoticeRefusal::CapExceeded { cap: CAP as u32 });
        }
        Ok(Book {
            pending,
            fired,
            held: self.held.clone(),
        })
    }

    /// Everything due by `now_ms`, each marked late or not, in time order; recorded as fired.
    pub(crate) fn take_due(&mut self, now_ms: u64) -> Vec<(AppNotice, bool)> {
        let at = |n: &AppNotice| n.fire_at_ms.unwrap_or(0);
        let (mut due, pending): (Vec<AppNotice>, Vec<AppNotice>) =
            self.pending.drain(..).partition(|n| at(n) <= now_ms);
        self.pending = pending;
        due.sort_by_key(at);
        self.fired.extend(due.iter().map(|n| Fired {
            app_id: n.app_id.clone(),
            id: n.id.clone(),
        }));
        due.into_iter()
            .map(|n| {
                let late = now_ms.saturating_sub(at(&n)) > LATE_AFTER_MS;
                (n, late)
            })
            .collect()
    }

    /// The next instant a reminder falls due.
    pub(crate) fn next_at(&self) -> Option<u64> {
        self.pending.iter().filter_map(|n| n.fire_at_ms).min()
    }

    /// Keep `notice`, fired with nothing to show it, to show once, late. The oldest goes past `CAP`.
    pub(crate) fn hold(&mut self, notice: AppNotice) {
        self.held.push(notice);
        if self.held.len() > CAP {
            self.held.remove(0);
        }
    }

    pub(crate) fn take_held(&mut self) -> Vec<AppNotice> {
        std::mem::take(&mut self.held)
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(self).map_err(|e| format!("the reminders could not be encoded: {e}"))
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, String> {
        serde_json::from_slice(bytes)
            .map_err(|e| format!("the stored reminders are unreadable: {e}"))
    }
}

#[cfg(test)]
#[path = "book_tests.rs"]
pub(crate) mod tests;
