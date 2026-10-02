//! The localhost connections a session is attached to.
//!
//! A session used to have ONE owning connection, an `Arc<AtomicUuid>` that
//! `ClaimSession` and a live sign-in re-pointed. So a second browser (or the
//! installed PWA, or another browser profile) could only take the session over,
//! and the window it took it from stopped receiving anything.
//!
//! This is an ordered set instead. Every member receives the session's
//! notifications; order matters only for the role each is told: the FIRST
//! member is the primary, the rest secondary (citadel-workspace
//! docs/plans/multi-window-sessions.md). When the primary leaves, the
//! longest-attached remaining member is promoted.
//!
//! Cheap to clone; clones share one set, so a route a long-lived task captured
//! at spawn sees every later attach and detach. The lock is never held across
//! an await and never while taking another lock.

use citadel_internal_service_types::SessionRole;
use parking_lot::RwLock;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone)]
pub(crate) struct SessionSubscribers {
    inner: Arc<RwLock<Attached>>,
}

struct Attached {
    /// Primary first, then in order of attachment.
    order: Vec<Uuid>,
    /// Who held the session when the last member DROPPED, so a reloaded
    /// browser's claim can adopt every session its old socket held. `None`
    /// after a release or once anyone holds it again.
    last_holder: Option<Uuid>,
}

/// What a detach did.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Detached {
    /// The connection was a member.
    pub removed: bool,
    /// It was the primary, and this member takes its place.
    pub promoted: Option<Uuid>,
}

impl SessionSubscribers {
    pub(crate) fn new(first: Uuid) -> Self {
        Self {
            inner: Arc::new(RwLock::new(Attached {
                order: vec![first],
                last_holder: None,
            })),
        }
    }

    pub(crate) fn members(&self) -> Vec<Uuid> {
        self.inner.read().order.clone()
    }

    pub(crate) fn primary(&self) -> Option<Uuid> {
        self.inner.read().order.first().copied()
    }

    pub(crate) fn contains(&self, connection: Uuid) -> bool {
        self.inner.read().order.contains(&connection)
    }

    /// Each member with its role, primary first.
    pub(crate) fn roles(&self) -> Vec<(Uuid, SessionRole)> {
        self.inner
            .read()
            .order
            .iter()
            .enumerate()
            .map(|(index, member)| {
                let role = if index == 0 {
                    SessionRole::Primary
                } else {
                    SessionRole::Secondary
                };
                (*member, role)
            })
            .collect()
    }

    /// The connection that held the session when the last member dropped.
    pub(crate) fn last_holder(&self) -> Option<Uuid> {
        self.inner.read().last_holder
    }

    /// Add `connection` after the existing members. `false` if it already was one.
    ///
    /// Authorization is the caller's: see `requests/connection_management_attach.rs`.
    pub(crate) fn attach(&self, connection: Uuid) -> bool {
        let mut inner = self.inner.write();
        if inner.order.contains(&connection) {
            return false;
        }
        inner.order.push(connection);
        inner.last_holder = None;
        true
    }

    /// The connection is gone: it leaves, and only it.
    pub(crate) fn detach(&self, connection: Uuid) -> Detached {
        self.leave(connection, true)
    }

    /// The connection is done with the session but is still connected.
    pub(crate) fn release(&self, connection: Uuid) -> Detached {
        self.leave(connection, false)
    }

    /// Make `caller` the only member, returning who it displaced.
    ///
    /// This is today's takeover (a live sign-in from an older UI) and today's
    /// orphan claim, unchanged. A caller that is already a member displaces
    /// nobody: re-asserting a session must not throw the other windows out.
    pub(crate) fn take_over(&self, caller: Uuid) -> Vec<Uuid> {
        let mut inner = self.inner.write();
        inner.last_holder = None;
        if inner.order.contains(&caller) {
            return Vec::new();
        }
        std::mem::replace(&mut inner.order, vec![caller])
    }

    fn leave(&self, connection: Uuid, remember: bool) -> Detached {
        let mut inner = self.inner.write();
        let Some(position) = inner.order.iter().position(|m| *m == connection) else {
            return Detached {
                removed: false,
                promoted: None,
            };
        };
        inner.order.remove(position);
        if inner.order.is_empty() {
            inner.last_holder = remember.then_some(connection);
        }
        let promoted = if position == 0 {
            inner.order.first().copied()
        } else {
            None
        };
        Detached {
            removed: true,
            promoted,
        }
    }
}

#[cfg(test)]
#[path = "session_subscribers_tests.rs"]
mod tests;
