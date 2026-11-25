//! Test-only: a local coturn (`turnserver`, e.g. `brew install coturn`) on loopback with one
//! long-term credential, modelled on Citadel-Protocol's `citadel_wire/tests/common/coturn.rs`
//! (UDP/TCP only: the agent tests do not exercise `turns:`).

use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub const USER: &str = "citadel";
pub const PASSWORD: &str = "relay-test-pass";
pub const REALM: &str = "citadel.test";

pub struct Coturn {
    child: Child,
    dir: PathBuf,
    pub port: u16,
}

/// A claimed port (crate::get_free_port) whose UDP side is free too: the UDP and TCP listeners
/// both bind it.
fn free_port() -> u16 {
    loop {
        let port = crate::get_free_port();
        if std::net::UdpSocket::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
}

impl Coturn {
    pub fn start() -> Self {
        let bin = which_turnserver();
        let dir =
            std::env::temp_dir().join(format!("citadel-agent-coturn-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let port = free_port();
        // Claimed for this process, like the listening port: no other test is handed them.
        let relay = crate::get_free_port_block();
        // A panicking test may exit before Drop runs; the watchdog kills turnserver once the test
        // process is gone (or on SIGTERM from Drop).
        const WATCHDOG: &str = "p=$1; shift; \"$@\" & c=$!; trap 'kill $c; exit 0' TERM; \
            while kill -0 \"$p\" 2>/dev/null && kill -0 $c 2>/dev/null; do sleep 1; done; kill $c";
        let child = Command::new("sh")
            .args(["-c", WATCHDOG, "sh", &std::process::id().to_string()])
            .arg(bin)
            .args(["-n", "--lt-cred-mech", "--fingerprint", "--no-tls"])
            .args(["--listening-ip", "127.0.0.1", "--relay-ip", "127.0.0.1"])
            .args(["--listening-port", &port.to_string()])
            .args(["--min-port", &relay.start.to_string()])
            .args(["--max-port", &(relay.end - 1).to_string()])
            .args(["--user", &format!("{USER}:{PASSWORD}"), "--realm", REALM])
            .args(["--allow-loopback-peers", "--simple-log"])
            .arg("--pidfile")
            .arg(dir.join("turnserver.pid"))
            .arg("--log-file")
            .arg(dir.join("turnserver.log"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn turnserver");
        let this = Self { child, dir, port };
        let deadline = Instant::now() + Duration::from_secs(10);
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        while TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_err() {
            assert!(Instant::now() < deadline, "coturn did not start listening");
            std::thread::sleep(Duration::from_millis(50));
        }
        this
    }

    pub fn url(&self, transport: &str) -> String {
        format!("turn:127.0.0.1:{}?transport={transport}", self.port)
    }

    /// coturn's own log (startup and errors; 4.18 does not log allocations by default).
    pub fn log(&self) -> String {
        std::fs::read_to_string(self.dir.join("turnserver.log")).unwrap_or_default()
    }
}

impl Drop for Coturn {
    fn drop(&mut self) {
        let _ = Command::new("kill")
            .arg(self.child.id().to_string())
            .status();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn which_turnserver() -> PathBuf {
    if let Ok(bin) = std::env::var("CITADEL_TURNSERVER_BIN") {
        return bin.into();
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .map(|d| d.join("turnserver"))
        .find(|p| p.is_file())
        .expect("coturn's turnserver not found on PATH (brew install coturn / apt install coturn)")
}
