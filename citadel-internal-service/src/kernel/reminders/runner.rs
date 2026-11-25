//! Each signed-in account's reminder runner: the only writer of its book (book.rs), so a sync
//! and a firing never race. It waits on tokio's clock, which does not advance while the computer
//! sleeps, for at most `RECHECK_MS`, and reads the wall clock again after every wait: what fell
//! due while the computer slept fires within `RECHECK_MS` of waking, once, marked late.

use super::book::Book;
use citadel_internal_service_types::{AppNotice, AppNoticeRefusal};
use citadel_sdk::logging::warn;
use futures::future::BoxFuture;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// The longest a runner waits before it reads the wall clock again.
pub(crate) const RECHECK_MS: u64 = 60_000;

pub(crate) trait WallClock: Send + Sync {
    /// Unix ms, UTC.
    fn now_ms(&self) -> u64;
}

pub(crate) struct SystemClock;

impl WallClock for SystemClock {
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| {
                u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Delivery {
    Shown,
    /// Neither the menu-bar app nor a window took it.
    Unreached,
}

/// What a runner reads and reaches (SBIO): the account's store, and who shows a notice.
pub(crate) trait ReminderIo: Send + Sync + 'static {
    fn load(&self, cid: u64) -> BoxFuture<'static, Result<Option<Vec<u8>>, String>>;
    fn save(&self, cid: u64, bytes: Vec<u8>) -> BoxFuture<'static, Result<(), String>>;
    /// Show `notice` through the menu-bar app if it listens, and tell the session's windows
    /// when `to_windows`.
    fn deliver(
        &self,
        cid: u64,
        notice: AppNotice,
        late: bool,
        to_windows: bool,
    ) -> BoxFuture<'static, Delivery>;
    fn signed_in(&self, cid: u64) -> bool;
}

pub(crate) struct SyncRequest {
    pub app_id: String,
    pub reminders: Vec<AppNotice>,
    pub reply: oneshot::Sender<Result<(), AppNoticeRefusal>>,
}

/// Run `cid`'s reminders until the account signs out or every sender is gone.
pub(crate) async fn run(
    cid: u64,
    io: Arc<dyn ReminderIo>,
    clock: Arc<dyn WallClock>,
    mut syncs: mpsc::UnboundedReceiver<SyncRequest>,
) {
    let mut book = match load(&*io, cid).await {
        Ok(book) => book,
        Err(e) => {
            warn!(target: "citadel", "[REMINDERS] {cid}: not run: {e}");
            return;
        }
    };
    loop {
        let now = clock.now_ms();
        let wait = book
            .next_at()
            .map_or(RECHECK_MS, |at| at.saturating_sub(now).min(RECHECK_MS));
        let synced = tokio::select! {
            sync = syncs.recv() => match sync {
                Some(sync) => { apply(&*io, cid, &mut book, sync).await; true }
                None => return,
            },
            _ = tokio::time::sleep(Duration::from_millis(wait)) => false,
        };
        if !io.signed_in(cid) {
            return;
        }
        fire(&*io, cid, &mut book, clock.now_ms(), synced).await;
    }
}

async fn load(io: &dyn ReminderIo, cid: u64) -> Result<Book, String> {
    match io.load(cid).await? {
        Some(bytes) => Book::decode(&bytes),
        None => Ok(Book::empty()),
    }
}

async fn apply(io: &dyn ReminderIo, cid: u64, book: &mut Book, sync: SyncRequest) {
    let outcome = match book.sync(&sync.app_id, sync.reminders) {
        Ok(next) => match next.encode() {
            Ok(bytes) => match io.save(cid, bytes).await {
                Ok(()) => {
                    *book = next;
                    Ok(())
                }
                Err(e) => Err(AppNoticeRefusal::Store(e)),
            },
            Err(e) => Err(AppNoticeRefusal::Store(e)),
        },
        Err(refused) => Err(refused),
    };
    // The window may have gone; the book stands either way.
    let _ = sync.reply.send(outcome);
}

/// Fire what is due and show what was held. Recorded as fired before it is shown: a crash in
/// between loses one reminder rather than showing it twice. Held notices go to the windows only
/// right after a sync, when a window has just said it is ready for them.
async fn fire(io: &dyn ReminderIo, cid: u64, book: &mut Book, now_ms: u64, windows_ready: bool) {
    let due = book.take_due(now_ms);
    let held = book.take_held();
    if due.is_empty() && held.is_empty() {
        return;
    }
    save(io, cid, book).await;
    let mut unreached = false;
    for (notice, late) in due {
        if io.deliver(cid, notice.clone(), late, true).await == Delivery::Unreached {
            book.hold(notice);
            unreached = true;
        }
    }
    for notice in held {
        if io.deliver(cid, notice.clone(), true, windows_ready).await == Delivery::Unreached {
            book.hold(notice);
            unreached = true;
        }
    }
    if unreached {
        save(io, cid, book).await;
    }
}

/// A failed save leaves the book in memory, which keeps deduplicating while this agent runs.
async fn save(io: &dyn ReminderIo, cid: u64, book: &Book) {
    let saved = match book.encode() {
        Ok(bytes) => io.save(cid, bytes).await,
        Err(e) => Err(e),
    };
    if let Err(e) = saved {
        warn!(target: "citadel", "[REMINDERS] {cid}: not saved: {e}");
    }
}

#[cfg(test)]
#[path = "runner_tests.rs"]
mod tests;
