//! The watchdog's decisions against a scripted machine. Time is virtual: `pause` advances the
//! clock, nothing sleeps.

use super::*;
use std::cell::{Cell, RefCell};

#[derive(Default)]
struct Script {
    parent_alive_for: u32,
    /// Probes before the new agent answers; `None` never.
    healthy_after: Option<u32>,
    child_exits: bool,
    spawn_fails: bool,
}

struct Fake {
    script: Script,
    clock: Cell<Instant>,
    probes: Cell<u32>,
    alive_checks: Cell<u32>,
    events: RefCell<Vec<String>>,
}

struct FakeChild {
    exits: bool,
}

impl Child for FakeChild {
    fn exited(&mut self) -> bool {
        self.exits
    }
    fn kill(&mut self) {}
}

impl Fake {
    fn new(script: Script) -> Self {
        Self {
            script,
            clock: Cell::new(Instant::now()),
            probes: Cell::new(0),
            alive_checks: Cell::new(0),
            events: RefCell::default(),
        }
    }
    fn events(&self) -> Vec<String> {
        self.events.borrow().clone()
    }
}

impl WatchIo for Fake {
    fn alive(&self, _pid: u32) -> bool {
        self.alive_checks.set(self.alive_checks.get() + 1);
        self.alive_checks.get() <= self.script.parent_alive_for
    }
    fn spawn(&self, program: &Path, _args: &[String]) -> Result<Box<dyn Child>, String> {
        let first = !self.events().iter().any(|e| e.starts_with("spawn"));
        self.events
            .borrow_mut()
            .push(format!("spawn {}", program.display()));
        if first && self.script.spawn_fails {
            return Err("exec format error".to_string());
        }
        Ok(Box::new(FakeChild {
            exits: first && self.script.child_exits,
        }))
    }
    fn healthy(&self, _addr: SocketAddr) -> bool {
        self.probes.set(self.probes.get() + 1);
        self.script
            .healthy_after
            .is_some_and(|n| self.probes.get() > n)
    }
    fn rename(&self, from: &Path, to: &Path) -> Result<(), String> {
        self.events
            .borrow_mut()
            .push(format!("rename {} {}", from.display(), to.display()));
        Ok(())
    }
    fn mark(&self, path: &Path) {
        self.events
            .borrow_mut()
            .push(format!("mark {}", path.display()));
    }
    fn now(&self) -> Instant {
        self.clock.get()
    }
    fn pause(&self) {
        self.clock
            .set(self.clock.get() + Duration::from_millis(250));
    }
    fn log(&self, _line: &str) {}
}

fn plan() -> WatchPlan {
    WatchPlan {
        parent_pid: 42,
        target: PathBuf::from("/a/citadel-agent"),
        backup: PathBuf::from("/a/.citadel-agent.previous"),
        health: "127.0.0.1:12345".parse().unwrap(),
        version: "0.9.0".to_string(),
        args: vec!["--bind".to_string(), "127.0.0.1:12345".to_string()],
        failed_marker: PathBuf::from("/c/failed-0.9.0"),
    }
}

const ROLLBACK: [&str; 4] = [
    "spawn /a/citadel-agent",
    "mark /c/failed-0.9.0",
    "rename /a/.citadel-agent.previous /a/citadel-agent",
    "spawn /a/citadel-agent",
];

#[test]
fn a_new_agent_that_answers_is_kept() {
    let io = Fake::new(Script {
        parent_alive_for: 3,
        healthy_after: Some(4),
        ..Default::default()
    });
    assert_eq!(supervise(&plan(), &io), Outcome::Healthy);
    assert_eq!(io.events(), vec!["spawn /a/citadel-agent"]);
}

#[test]
fn a_new_agent_that_never_answers_is_rolled_back_after_the_deadline() {
    let io = Fake::new(Script::default());
    let started = io.now();
    assert_eq!(supervise(&plan(), &io), Outcome::RolledBack);
    assert_eq!(io.events(), ROLLBACK);
    assert!(io.now() - started >= HEALTHY_WITHIN);
}

#[test]
fn a_new_agent_that_exits_is_rolled_back_at_once() {
    let io = Fake::new(Script {
        child_exits: true,
        ..Default::default()
    });
    assert_eq!(supervise(&plan(), &io), Outcome::RolledBack);
    assert_eq!(io.events(), ROLLBACK);
    assert_eq!(io.probes.get(), 0);
}

#[test]
fn a_new_agent_that_will_not_start_is_rolled_back() {
    let io = Fake::new(Script {
        spawn_fails: true,
        ..Default::default()
    });
    assert_eq!(supervise(&plan(), &io), Outcome::RolledBack);
    assert_eq!(io.events(), ROLLBACK);
}

#[test]
fn an_old_agent_that_never_exits_gets_its_binary_back_and_nothing_starts() {
    let io = Fake::new(Script {
        parent_alive_for: u32::MAX,
        healthy_after: Some(0),
        ..Default::default()
    });
    assert_eq!(supervise(&plan(), &io), Outcome::ParentStuck);
    assert_eq!(io.events(), ROLLBACK[1..3]);
}

#[test]
fn the_plan_survives_the_environment_variable() {
    let encoded = serde_json::to_string(&plan()).unwrap();
    assert_eq!(serde_json::from_str::<WatchPlan>(&encoded).unwrap(), plan());
}
