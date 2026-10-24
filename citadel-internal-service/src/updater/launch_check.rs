//! Whether a staged agent would start with this agent's command (unix): `staged
//! --check-launch <relaunch args>`, which the agent answers once every setting it requires
//! resolves, before it binds anything. A self-replacing install restarts with the command it
//! was started with; a version that requires a setting that command lacks would refuse to
//! start, be rolled back by the watchdog, and sign everyone out for nothing. Asked first, it is
//! never installed, and the member is told why.
//!
//! Every release from the one that added this answers `--check-launch`; one that stopped would
//! never be installable by a self-replacing agent, which this would report as its refusal.

use super::watch_os::detached;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

pub const CHECK_LAUNCH_FLAG: &str = "--check-launch";
/// The check resolves settings and exits; one that has not answered by then is not an agent
/// that would start.
const CHECK_WITHIN: Duration = Duration::from_secs(30);
const POLL_EVERY: Duration = Duration::from_millis(50);

/// `Err` with what `staged` printed when it would not start with `args`.
pub fn check_launch(staged: &Path, args: &[String]) -> Result<(), String> {
    let log = staged.with_extension("check-launch.stderr");
    let stderr = std::fs::File::create(&log).map_err(|e| format!("{}: {e}", log.display()))?;
    let checked: Vec<String> = std::iter::once(CHECK_LAUNCH_FLAG.to_string())
        .chain(args.iter().cloned())
        .collect();
    let mut child = detached(staged, &checked)
        .stdout(Stdio::null())
        .stderr(stderr)
        .spawn()
        .map_err(|e| format!("{}: {e}", staged.display()))?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => break status,
            None if started.elapsed() >= CHECK_WITHIN => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "the new version did not answer {CHECK_LAUNCH_FLAG} within {CHECK_WITHIN:?}"
                ));
            }
            None => std::thread::sleep(POLL_EVERY),
        }
    };
    let said = std::fs::read_to_string(&log).unwrap_or_default();
    let _ = std::fs::remove_file(&log);
    if status.success() {
        return Ok(());
    }
    Err(format!(
        "the new version would not start with the command this agent was started with ({status}): {}",
        said.trim()
    ))
}
