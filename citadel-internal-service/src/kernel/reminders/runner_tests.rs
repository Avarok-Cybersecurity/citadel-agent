//! The runner on tokio's paused clock, with a wall clock the tests move on their own: moving
//! the wall clock alone is the computer sleeping (tokio's clock stands still then).

use super::*;
use crate::kernel::reminders::book::tests::reminder;
use crate::kernel::reminders::book::CAP;
use crate::kernel::reminders::Reminders;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const CID: u64 = 7;
const T0: u64 = 1_800_000_000_000;
const CAL: &str = "net.avarok.calendar";

struct Clock(AtomicU64);

impl WallClock for Clock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// `shown` is what was delivered: (id, late, to_windows).
struct Fake {
    store: Mutex<Option<Vec<u8>>>,
    shown: Mutex<Vec<(String, bool, bool)>>,
    reachable: AtomicBool,
    signed_in: AtomicBool,
}

impl ReminderIo for Fake {
    fn load(&self, _: u64) -> BoxFuture<'static, Result<Option<Vec<u8>>, String>> {
        let stored = self.store.lock().clone();
        Box::pin(async move { Ok(stored) })
    }
    fn save(&self, _: u64, bytes: Vec<u8>) -> BoxFuture<'static, Result<(), String>> {
        *self.store.lock() = Some(bytes);
        Box::pin(async { Ok(()) })
    }
    fn deliver(
        &self,
        _: u64,
        n: AppNotice,
        late: bool,
        to_windows: bool,
    ) -> BoxFuture<'static, Delivery> {
        let delivery = if self.reachable.load(Ordering::SeqCst) {
            self.shown.lock().push((n.id, late, to_windows));
            Delivery::Shown
        } else {
            Delivery::Unreached
        };
        Box::pin(async move { delivery })
    }
    fn signed_in(&self, _: u64) -> bool {
        self.signed_in.load(Ordering::SeqCst)
    }
}

struct Rig {
    io: Arc<Fake>,
    clock: Arc<Clock>,
    reminders: Reminders,
}

impl Rig {
    fn new() -> Self {
        let io = Arc::new(Fake {
            store: Mutex::new(None),
            shown: Mutex::new(Vec::new()),
            reachable: AtomicBool::new(true),
            signed_in: AtomicBool::new(true),
        });
        Self::over(io)
    }

    /// A fresh agent over `io`'s store: what an agent restart is.
    fn over(io: Arc<Fake>) -> Self {
        let clock = Arc::new(Clock(AtomicU64::new(T0)));
        let reminders = Reminders::new(clock.clone());
        Self {
            io,
            clock,
            reminders,
        }
    }

    async fn sync(&self, list: Vec<AppNotice>) -> Result<(), AppNoticeRefusal> {
        let io: Arc<dyn ReminderIo> = self.io.clone();
        let out = self.reminders.sync(io, CID, CAL.to_string(), list).await;
        settle().await;
        out
    }

    /// Both clocks move: the computer is awake.
    async fn pass(&self, ms: u64) {
        self.clock.0.fetch_add(ms, Ordering::SeqCst);
        tokio::time::advance(Duration::from_millis(ms)).await;
        settle().await;
    }

    /// Only the wall clock moves: the computer slept.
    fn sleep(&self, ms: u64) {
        self.clock.0.fetch_add(ms, Ordering::SeqCst);
    }

    fn shown(&self) -> Vec<(String, bool, bool)> {
        self.io.shown.lock().clone()
    }
}

async fn settle() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
}

fn at(id: &str, ms_from_t0: u64) -> AppNotice {
    reminder(id, T0 + ms_from_t0)
}

fn shown(id: &str, late: bool) -> (String, bool, bool) {
    (id.to_string(), late, true)
}

#[tokio::test(start_paused = true)]
async fn a_reminder_fires_at_its_time_and_not_before() {
    let rig = Rig::new();
    rig.sync(vec![at("a", 5_000)]).await.unwrap();
    rig.pass(4_999).await;
    assert!(rig.shown().is_empty());
    rig.pass(1).await;
    assert_eq!(rig.shown(), vec![shown("a", false)]);
    rig.pass(RECHECK_MS * 3).await;
    assert_eq!(rig.shown().len(), 1, "fired once");
}

#[tokio::test(start_paused = true)]
async fn a_sync_without_it_cancels_a_reminder() {
    let rig = Rig::new();
    rig.sync(vec![at("a", 5_000), at("b", 6_000)])
        .await
        .unwrap();
    rig.sync(vec![at("b", 6_000)]).await.unwrap();
    rig.pass(10_000).await;
    assert_eq!(rig.shown(), vec![shown("b", false)]);
}

#[tokio::test(start_paused = true)]
async fn reminders_survive_an_agent_restart() {
    let first = Rig::new();
    first.sync(vec![at("a", 5_000)]).await.unwrap();
    first.io.signed_in.store(false, Ordering::SeqCst);
    first.pass(1_000).await;
    let io = first.io.clone();
    drop(first);
    io.signed_in.store(true, Ordering::SeqCst);
    let second = Rig::over(io);
    second.reminders.start(second.io.clone(), CID);
    second.pass(5_000).await;
    assert_eq!(second.shown(), vec![shown("a", false)]);
}

#[tokio::test(start_paused = true)]
async fn what_fired_before_a_restart_never_fires_again() {
    let first = Rig::new();
    first.sync(vec![at("a", 1_000)]).await.unwrap();
    first.pass(1_000).await;
    let io = first.io.clone();
    drop(first);
    let second = Rig::over(io);
    second.sync(vec![at("a", 1_000)]).await.unwrap();
    second.pass(RECHECK_MS * 2).await;
    assert_eq!(second.shown().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn missed_while_asleep_fires_once_on_waking_marked_late() {
    let rig = Rig::new();
    rig.sync(vec![at("a", 30 * 60_000)]).await.unwrap();
    rig.sleep(8 * 3_600_000);
    rig.pass(RECHECK_MS).await;
    assert_eq!(rig.shown(), vec![shown("a", true)]);
    rig.pass(RECHECK_MS * 3).await;
    assert_eq!(rig.shown().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_resync_or_a_repeat_shows_one_notice() {
    let rig = Rig::new();
    rig.sync(vec![at("a", 1_000), at("a", 1_000)])
        .await
        .unwrap();
    rig.pass(1_000).await;
    rig.sync(vec![at("a", 1_000)]).await.unwrap();
    rig.pass(RECHECK_MS).await;
    assert_eq!(rig.shown(), vec![shown("a", false)]);
}

#[tokio::test(start_paused = true)]
async fn past_the_cap_the_sync_is_refused_and_nothing_changes() {
    let rig = Rig::new();
    rig.sync(vec![at("keep", 1_000)]).await.unwrap();
    let many: Vec<AppNotice> = (0..=CAP as u64)
        .map(|i| at(&format!("r{i}"), 2_000 + i))
        .collect();
    let refused = rig.sync(many).await;
    assert_eq!(
        refused,
        Err(AppNoticeRefusal::CapExceeded { cap: CAP as u32 })
    );
    rig.pass(5_000).await;
    assert_eq!(rig.shown(), vec![shown("keep", false)]);
}

#[tokio::test(start_paused = true)]
async fn what_nothing_could_show_is_shown_once_at_the_next_sync_marked_late() {
    let rig = Rig::new();
    rig.sync(vec![at("a", 1_000)]).await.unwrap();
    rig.io.reachable.store(false, Ordering::SeqCst);
    rig.pass(1_000).await;
    assert!(rig.shown().is_empty());
    rig.io.reachable.store(true, Ordering::SeqCst);
    rig.sync(Vec::new()).await.unwrap();
    rig.sync(Vec::new()).await.unwrap();
    assert_eq!(rig.shown(), vec![shown("a", true)]);
}

#[tokio::test(start_paused = true)]
async fn a_held_notice_reaches_the_menu_bar_app_without_a_window() {
    let rig = Rig::new();
    rig.sync(vec![at("a", 1_000)]).await.unwrap();
    rig.io.reachable.store(false, Ordering::SeqCst);
    rig.pass(1_000).await;
    rig.io.reachable.store(true, Ordering::SeqCst);
    rig.pass(RECHECK_MS).await;
    assert_eq!(rig.shown(), vec![("a".to_string(), true, false)]);
}

#[tokio::test(start_paused = true)]
async fn a_signed_out_account_fires_nothing() {
    let rig = Rig::new();
    rig.sync(vec![at("a", 1_000)]).await.unwrap();
    rig.io.signed_in.store(false, Ordering::SeqCst);
    rig.pass(RECHECK_MS * 2).await;
    assert!(rig.shown().is_empty());
}
