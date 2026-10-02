//! The watchdog with its real I/O (processes, renames, a loopback probe), on scratch files.
//! The "agents" are shell scripts; the healthy one is a python listener that accepts the
//! watchdog's probe and exits.
#![cfg(unix)]

use citadel_internal_service::updater::watch_os::OsWatch;
use citadel_internal_service::updater::watchdog::{supervise, Outcome, WatchPlan};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A pid that has exited: the old agent, already gone.
fn gone_pid() -> u32 {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn plan(dir: &Path, port: u16) -> WatchPlan {
    WatchPlan {
        parent_pid: gone_pid(),
        target: dir.join("citadel-agent"),
        backup: dir.join(".citadel-agent.previous"),
        health: format!("127.0.0.1:{port}").parse().unwrap(),
        version: "0.9.0".to_string(),
        args: Vec::new(),
        failed_marker: dir.join("failed-0.9.0"),
    }
}

#[test]
fn a_new_agent_that_exits_at_once_is_replaced_by_the_old_one() {
    let dir = tempfile::tempdir().unwrap();
    let plan = plan(dir.path(), free_port());
    script(&plan.target, "exit 3");
    script(&plan.backup, "exit 0 # old");
    assert_eq!(supervise(&plan, &OsWatch), Outcome::RolledBack);
    let target = std::fs::read_to_string(&plan.target).unwrap();
    assert!(target.contains("# old"), "the old agent is back: {target}");
    assert!(!plan.backup.exists());
    assert!(
        plan.failed_marker.exists(),
        "the version is not retried automatically"
    );
}

#[test]
fn a_new_agent_that_answers_on_its_socket_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let plan = plan(dir.path(), port);
    let listener = format!(
        "exec python3 -c 'import socket; s=socket.socket(); \
         s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); \
         s.bind((\"127.0.0.1\", {port})); s.listen(1); s.accept()' # new"
    );
    script(&plan.target, &listener);
    script(&plan.backup, "exit 0 # old");
    assert_eq!(supervise(&plan, &OsWatch), Outcome::Healthy);
    assert!(std::fs::read_to_string(&plan.target)
        .unwrap()
        .contains("# new"));
    assert!(
        plan.backup.exists(),
        "the backup is kept until the next update"
    );
    assert!(!plan.failed_marker.exists());
}
