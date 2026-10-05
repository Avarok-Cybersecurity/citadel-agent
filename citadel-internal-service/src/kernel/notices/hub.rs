//! Who hears native notices, and which windows have what in front of the user.

use crate::kernel::session_route::{deliver, Clients};
use citadel_internal_service_types::{InternalServiceResponse, NativeNotice, NoticeRows};
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use uuid::Uuid;

/// Where a notice goes once decided (SBIO): the menu-bar app's stream here,
/// a desktop backend later.
pub(crate) trait Notifier: Send + Sync {
    fn notify(&self, notice: &NativeNotice);
    fn rows(&self, rows: &NoticeRows);
}

/// The secret the native app put in the agent's environment when it started
/// it. Never printed: its Debug says only whether there is one.
pub struct NoticeToken(Vec<u8>);

impl NoticeToken {
    /// `None` for an empty value: an empty token would admit anybody.
    pub fn new(secret: String) -> Option<Self> {
        (!secret.is_empty()).then(|| Self(secret.into_bytes()))
    }
}

impl std::fmt::Debug for NoticeToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NoticeToken(<redacted>)")
    }
}

/// A connection that is gone needs no cleanup here: subscribers and focus are
/// read through the live connection map, and a closed window drops out of both.
pub(crate) struct NoticeHub {
    token: Option<NoticeToken>,
    clients: Clients,
    subscribers: Subscribers,
    /// connection -> (account, conversation it shows, if any)
    focus: RwLock<HashMap<Uuid, (u64, Option<u64>)>>,
    notifiers: Vec<Arc<dyn Notifier>>,
    /// What the declared windows were last told `is_heard` is.
    told_heard: Mutex<bool>,
}

type Subscribers = Arc<RwLock<HashSet<Uuid>>>;

/// The menu-bar app's stream: every connection that subscribed with the token.
struct StreamNotifier {
    subscribers: Subscribers,
    clients: Clients,
}

impl StreamNotifier {
    fn send(&self, response: InternalServiceResponse) {
        stream_send(&self.subscribers, &self.clients, response);
    }
}

/// Send to every live subscriber, forgetting the ones that are gone; how many it reached.
fn stream_send(
    subscribers: &Subscribers,
    clients: &Clients,
    response: InternalServiceResponse,
) -> usize {
    let live: Vec<Uuid> = {
        let connected = clients.read();
        let mut subscribers = subscribers.write();
        subscribers.retain(|id| connected.contains_key(id));
        subscribers.iter().copied().collect()
    };
    deliver(clients, &live, response).len()
}

impl Notifier for StreamNotifier {
    fn notify(&self, notice: &NativeNotice) {
        self.send(InternalServiceResponse::NativeNotice(Box::new(
            notice.clone(),
        )));
    }
    fn rows(&self, rows: &NoticeRows) {
        self.send(InternalServiceResponse::NoticeRows(rows.clone()));
    }
}

impl NoticeHub {
    /// `token` is what the native app passed at launch; without one the notice
    /// plane admits nobody. Notices go to the subscribed stream, and to
    /// `others` (a desktop backend, or a test's recorder).
    pub(crate) fn new(
        token: Option<NoticeToken>,
        clients: Clients,
        others: Vec<Arc<dyn Notifier>>,
    ) -> Self {
        let subscribers: Subscribers = Arc::default();
        let stream: Arc<dyn Notifier> = Arc::new(StreamNotifier {
            subscribers: subscribers.clone(),
            clients: clients.clone(),
        });
        let notifiers: Vec<Arc<dyn Notifier>> = std::iter::once(stream).chain(others).collect();
        Self {
            token,
            clients,
            subscribers,
            focus: RwLock::default(),
            notifiers,
            told_heard: Mutex::new(false),
        }
    }

    /// Whether `presented` is the launch token. Constant time; no token
    /// configured admits nobody.
    pub(crate) fn admits(&self, presented: &str) -> bool {
        let recorded: Option<Vec<u8>> = self.token.as_ref().map(|t| t.0.clone());
        crate::kernel::credential_fingerprint::matches(
            recorded.as_ref(),
            Some(&presented.as_bytes().to_vec()),
        )
    }

    pub(crate) fn subscribe(&self, connection: Uuid) {
        self.subscribers.write().insert(connection);
    }

    /// Whether anything would hear a notice: the stream has a subscriber, or
    /// another notifier is installed.
    pub(crate) fn is_heard(&self) -> bool {
        let clients = self.clients.read();
        self.subscribers
            .read()
            .iter()
            .any(|id| clients.contains_key(id))
            || self.notifiers.len() > 1
    }

    pub(crate) fn report_focus(
        &self,
        connection: Uuid,
        cid: u64,
        peer: Option<u64>,
        focused: bool,
    ) {
        let mut focus = self.focus.write();
        if focused {
            focus.insert(connection, (cid, peer));
        } else if focus.get(&connection).is_some_and(|(held, _)| *held == cid) {
            focus.remove(&connection);
        }
    }

    /// `is_heard` now, if it is not what the windows were last told; recorded as told.
    ///
    /// Read and recorded under one lock, so two callers racing on one change
    /// cannot both report it, nor report it out of order.
    pub(crate) fn heard_changed(&self) -> Option<bool> {
        let mut told = self.told_heard.lock();
        let now = self.is_heard();
        (now != *told).then(|| {
            *told = now;
            now
        })
    }

    /// What every focused, still-connected window of `cid` shows.
    pub(crate) fn focused_on(&self, cid: u64) -> Vec<Option<u64>> {
        let clients = self.clients.read();
        let mut focus = self.focus.write();
        focus.retain(|id, _| clients.contains_key(id));
        focus
            .values()
            .filter(|(held, _)| *held == cid)
            .map(|(_, peer)| *peer)
            .collect()
    }

    /// Send `response` to the menu-bar app's stream alone; how many subscribers it reached.
    pub(crate) fn send_to_stream(&self, response: InternalServiceResponse) -> usize {
        stream_send(&self.subscribers, &self.clients, response)
    }

    /// Whether the menu-bar app's stream has a live subscriber.
    pub(crate) fn has_stream(&self) -> bool {
        let clients = self.clients.read();
        self.subscribers
            .read()
            .iter()
            .any(|id| clients.contains_key(id))
    }

    pub(crate) fn send_notice(&self, notice: &NativeNotice) {
        for notifier in &self.notifiers {
            notifier.notify(notice);
        }
    }

    pub(crate) fn send_rows(&self, rows: &NoticeRows) {
        for notifier in &self.notifiers {
            notifier.rows(rows);
        }
    }
}

#[cfg(test)]
#[path = "hub_tests.rs"]
mod tests;
