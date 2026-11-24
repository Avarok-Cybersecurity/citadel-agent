//! Test ports nobody else can be handed.

use std::fs::{File, OpenOptions, TryLockError};
use std::net::TcpListener;
use std::ops::Range;
use std::sync::Mutex;

/// Ports below every OS's ephemeral range, in blocks a test process claims for as long as
/// it lives, so no two live test processes can be handed the same one.
///
/// A test asks for a port and binds it later, so between the two the port is bound by
/// nobody, and nothing a probe sees can show that another process was just handed it.
/// nextest gives each test its own process, so no in-process registry can see that
/// either. Two properties close it:
///
/// 1. Ports come from a fixed range *below* the ephemeral ranges (Linux 32768, macOS and
///    Windows 49152), so a `bind(:0)` or an outgoing connection anywhere on the machine is
///    never given one of ours.
/// 2. A process is handed ports only from blocks it has claimed, and a claim is an
///    exclusive OS lock on that block's file, held until the process ends. The OS drops
///    the lock however the process ends, so a block is never left claimed by nobody.
///
/// (2) used to be arithmetic: block = `pid % 750`, on the reasoning that nextest's
/// processes have near-consecutive pids. That holds on Linux. On Windows pids are multiples
/// of four, reused from anywhere in a range of about ten thousand, so two live test
/// processes 1500 apart shared a block often enough to fail CI about once a day with
/// `AddrInUse` (os error 10048): both started at the block's first port, the second took
/// whichever port the first had been handed and not yet bound, and one of the two binds
/// lost. The pid now only spreads out where processes start looking.
///
/// The bind probe is a backstop for a port held by something that is not one of these
/// tests. It closes the socket before returning; (1) and (2) are what make that harmless.
const PORT_BASE: u16 = 20_000;
const PORTS_PER_BLOCK: u16 = 16;
const BLOCKS: u16 = 750; // 750 * 16 = 12_000 ports, ending at 32_000.
const PORT_END: u16 = PORT_BASE + BLOCKS * PORTS_PER_BLOCK;

/// A port no other live test process has been or will be handed.
pub fn get_free_port() -> u16 {
    static PORTS: Mutex<Option<PortAllocator>> = Mutex::new(None);
    PORTS
        .lock()
        .expect("an earlier port allocation panicked")
        .get_or_insert_with(|| PortAllocator::for_pid(std::process::id()))
        .next_port()
}

/// One process's ports: the blocks it has claimed and what is left of the newest.
struct PortAllocator {
    next_block: u16,
    unused: Range<u16>,
    /// Held, never read: dropping a file releases its block to other processes.
    claims: Vec<File>,
}

impl PortAllocator {
    fn for_pid(pid: u32) -> Self {
        Self {
            next_block: (pid % u32::from(BLOCKS)) as u16,
            unused: 0..0,
            claims: Vec::new(),
        }
    }

    fn next_port(&mut self) -> u16 {
        loop {
            let Some(port) = self.unused.next() else {
                self.unused = self.claim_a_block();
                continue;
            };
            if TcpListener::bind(("127.0.0.1", port)).is_ok() {
                return port;
            }
        }
    }

    /// Claims the first block from `next_block` on that no live process holds.
    fn claim_a_block(&mut self) -> Range<u16> {
        for _ in 0..BLOCKS {
            let block = self.next_block;
            self.next_block = (block + 1) % BLOCKS;
            if let Some(claim) = claim(block) {
                self.claims.push(claim);
                let first = PORT_BASE + block * PORTS_PER_BLOCK;
                return first..first + PORTS_PER_BLOCK;
            }
        }
        panic!("every port block in {PORT_BASE}..{PORT_END} is claimed by a live test process");
    }
}

/// The exclusive lock on `block`'s file, or `None` while another holder has it.
fn claim(block: u16) -> Option<File> {
    let dir = std::env::temp_dir().join("citadel-test-port-claims");
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|err| panic!("creating the port claims in {}: {err}", dir.display()));
    let path = dir.join(format!("{block}.lock"));
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .unwrap_or_else(|err| panic!("opening the port claim {}: {err}", path.display()));
    match file.try_lock() {
        Ok(()) => Some(file),
        Err(TryLockError::WouldBlock) => None,
        Err(TryLockError::Error(err)) => {
            panic!("locking the port claim {}: {err}", path.display())
        }
    }
}

#[cfg(test)]
#[path = "ports_tests.rs"]
mod tests;
