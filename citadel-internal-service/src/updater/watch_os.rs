//! The watchdog's real I/O (unix): processes, the loopback probe, renames.

use super::watchdog::{Child, WatchIo, WATCHDOG_ENV};
use std::net::{SocketAddr, TcpStream};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const PROBE_EVERY: Duration = Duration::from_millis(250);
const CONNECT_WITHIN: Duration = Duration::from_secs(1);

pub struct OsWatch;

struct OsChild(std::process::Child);

impl Child for OsChild {
    fn exited(&mut self) -> bool {
        !matches!(self.0.try_wait(), Ok(None))
    }
    fn kill(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Starts `program` in a process group of its own, so the old agent's terminal or launcher
/// ending does not take it along, and without the watchdog's variable.
pub fn detached(program: &Path, args: &[String]) -> Command {
    let mut command = Command::new(program);
    command
        .args(args)
        .env_remove(WATCHDOG_ENV)
        .stdin(Stdio::null())
        .process_group(0);
    command
}

impl WatchIo for OsWatch {
    fn alive(&self, pid: u32) -> bool {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        // Signal 0 checks for the process without sending anything.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    fn spawn(&self, program: &Path, args: &[String]) -> Result<Box<dyn Child>, String> {
        detached(program, args)
            .spawn()
            .map(|c| Box::new(OsChild(c)) as Box<dyn Child>)
            .map_err(|e| format!("{}: {e}", program.display()))
    }

    fn healthy(&self, addr: SocketAddr) -> bool {
        TcpStream::connect_timeout(&addr, CONNECT_WITHIN).is_ok()
    }

    fn rename(&self, from: &Path, to: &Path) -> Result<(), String> {
        std::fs::rename(from, to)
            .map_err(|e| format!("{} -> {}: {e}", from.display(), to.display()))
    }

    fn mark(&self, path: &Path) {
        if let Err(e) = std::fs::write(path, b"") {
            self.log(&format!("{} was not written: {e}", path.display()));
        }
    }

    fn now(&self) -> Instant {
        Instant::now()
    }

    fn pause(&self) {
        std::thread::sleep(PROBE_EVERY);
    }

    fn log(&self, line: &str) {
        eprintln!("[citadel-agent watchdog] {line}");
    }
}
