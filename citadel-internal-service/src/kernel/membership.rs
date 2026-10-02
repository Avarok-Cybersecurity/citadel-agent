//! Changing which connections a session is attached to, and saying so.
//!
//! Every change of membership goes through here, so the role announcement
//! (kernel/session_route.rs `announce_roles`) cannot be forgotten at one site.
//! Locks: the connection map is read and released before any send.

use crate::kernel::session_route::{announce_roles, Clients};
use crate::kernel::session_subscribers::SessionSubscribers;
use crate::kernel::{CitadelWorkspaceService, Connection};
use citadel_internal_service_types::ClientCapabilities;
use citadel_sdk::prelude::Ratchet;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

/// The session's subscriber set, if the session exists.
pub(crate) fn subscribers_of<T, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
) -> Option<SessionSubscribers> {
    this.server_connection_map
        .read()
        .get(&cid)
        .map(|conn| conn.subscribers.clone())
}

/// Make `caller` the only subscriber -- today's takeover and orphan claim --
/// telling each displaced connection it is detached. A live `Connect` takes
/// over this way, for UIs that predate `AttachSession`. A caller already attached
/// displaces nobody. `false` if there is no such session.
pub(crate) fn take_over<T, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    caller: Uuid,
) -> bool {
    let Some(subscribers) = subscribers_of(this, cid) else {
        return false;
    };
    take_over_in(&this.tx_to_localhost_clients, cid, &subscribers, caller);
    true
}

pub(crate) fn take_over_in(
    clients: &Clients,
    cid: u64,
    subscribers: &SessionSubscribers,
    caller: Uuid,
) {
    let displaced = subscribers.take_over(caller);
    if !displaced.is_empty() {
        announce_roles(clients, cid, subscribers, &displaced);
    }
}

/// A connection closed: it leaves every session it was attached to, and only
/// it. Each session that still has members hears the new roles.
/// A localhost connection is gone. It leaves every session it was attached to,
/// and only it: a session keeps its other windows, and a primary that left is
/// replaced by the longest-attached one.
pub(crate) fn connection_closed<R: Ratchet>(
    capabilities: &RwLock<HashMap<Uuid, ClientCapabilities>>,
    sessions: &Arc<RwLock<HashMap<u64, Connection<R>>>>,
    clients: &Clients,
    connection: Uuid,
) {
    capabilities.write().remove(&connection);
    detach_everywhere(sessions, clients, connection);
}

pub(crate) fn detach_everywhere<R: Ratchet>(
    sessions: &Arc<RwLock<HashMap<u64, Connection<R>>>>,
    clients: &Clients,
    connection: Uuid,
) {
    let attached: Vec<(u64, SessionSubscribers)> = sessions
        .read()
        .iter()
        .map(|(cid, conn)| (*cid, conn.subscribers.clone()))
        .collect();
    for (cid, subscribers) in attached {
        leave_and_announce(
            clients,
            cid,
            &subscribers,
            subscribers.detach(connection).removed,
        );
    }
}

/// `connection` is done with `cid` but stays connected (`ReleaseSession`).
pub(crate) fn release<T, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    connection: Uuid,
) {
    if let Some(subscribers) = subscribers_of(this, cid) {
        let removed = subscribers.release(connection).removed;
        leave_and_announce(&this.tx_to_localhost_clients, cid, &subscribers, removed);
    }
}

fn leave_and_announce(
    clients: &Clients,
    cid: u64,
    subscribers: &SessionSubscribers,
    removed: bool,
) {
    // A session left with nobody is an orphan: nobody to tell. One left with
    // members hears its roles, whether or not the primary changed -- the count
    // changed for everyone.
    if removed && subscribers.primary().is_some() {
        announce_roles(clients, cid, subscribers, &[]);
    }
}
