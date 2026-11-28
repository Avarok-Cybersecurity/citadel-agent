//! Apps' reminders, fired by the agent with no window open (app notices).
//!
//! - book.rs: one session's reminders, what fired and what waits to be shown (pure).
//! - runner.rs: each account's runner, the only writer of its book; the clock and the store.
//! - io.rs: the agent's side: the account's own store, the menu-bar app and the windows.
//!
//! The page holds the plans and syncs them whole (`AppRemindersSync`); the agent holds the times.
//! A runner starts when a session is hosted (`host_ilm_for`) and ends when it signs out.

pub(crate) mod book;
mod io;
pub(crate) mod runner;

use citadel_internal_service_types::{AppNotice, AppNoticeRefusal};
use parking_lot::Mutex;
use runner::{ReminderIo, SyncRequest, WallClock};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

pub(crate) struct Reminders {
    clock: Arc<dyn WallClock>,
    runners: Mutex<HashMap<u64, mpsc::UnboundedSender<SyncRequest>>>,
}

impl Reminders {
    pub(crate) fn new(clock: Arc<dyn WallClock>) -> Self {
        Self {
            clock,
            runners: Mutex::default(),
        }
    }

    /// `cid`'s runner, started if it is not running.
    pub(crate) fn start(
        &self,
        io: Arc<dyn ReminderIo>,
        cid: u64,
    ) -> mpsc::UnboundedSender<SyncRequest> {
        let mut runners = self.runners.lock();
        if let Some(running) = runners.get(&cid).filter(|tx| !tx.is_closed()) {
            return running.clone();
        }
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::task::spawn(runner::run(cid, io, self.clock.clone(), rx));
        runners.insert(cid, tx.clone());
        tx
    }

    /// Replace `cid`'s reminders for `app_id`; answered once they are stored.
    pub(crate) async fn sync(
        &self,
        io: Arc<dyn ReminderIo>,
        cid: u64,
        app_id: String,
        reminders: Vec<AppNotice>,
    ) -> Result<(), AppNoticeRefusal> {
        let stopped =
            || AppNoticeRefusal::Store("the account's reminders could not be read".to_string());
        let (reply, answer) = oneshot::channel();
        self.start(io, cid)
            .send(SyncRequest {
                app_id,
                reminders,
                reply,
            })
            .map_err(|_| stopped())?;
        answer.await.map_err(|_| stopped())?
    }
}
