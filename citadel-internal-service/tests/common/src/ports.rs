//! Test ports nobody else can be handed.

use std::net::TcpListener;

/// Ports below the OS ephemeral range, partitioned so concurrent test processes
/// cannot be handed the same one.
///
/// The obvious implementation — bind `:0`, read the port, drop the listener — is
/// what this used to be, and it is a race. Between the drop and the caller's real
/// bind the port is free, so the kernel may hand that exact port to another test
/// process binding `:0`. That is the `AddrInUse` that took out
/// `group_chat::test_internal_service_group_create` in CI at 0.012s, before a
/// single line of group logic ran. nextest gives each test its own process, so no
/// process-local registry can see the conflict; it has to be avoided by
/// construction.
///
/// Two properties do that:
///
/// 1. Ports come from a fixed range *below* Linux's ephemeral range
///    (`net.ipv4.ip_local_port_range`, 32768-60999 by default). A `bind(:0)`
///    anywhere on the machine can never be handed one of ours, which removes the
///    race entirely for the case above.
/// 2. Concurrent test processes are separated by partitioning that range by pid
///    into disjoint blocks. nextest spawns processes with near-consecutive pids,
///    so multiplying by the block size is what keeps neighbours from overlapping:
///    `pid` and `pid + 1` land [`PORTS_PER_PROCESS`] apart, not one apart.
///
/// The bindability probe below is a backstop, not the mechanism — it catches a
/// port a previous run still holds or one sitting in TIME_WAIT. It still closes
/// the socket before returning, so it carries the same race it always did; what
/// makes that harmless now is (1) and (2).
const PORT_BASE: u16 = 20_000;
const PORTS_PER_PROCESS: u16 = 16;
const PORT_SLOTS: u16 = 750; // 750 * 16 = 12_000 ports, ending at 32_000.

/// Helper function to get a free port for testing
pub fn get_free_port() -> u16 {
    static NEXT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);
    const SPAN: u16 = PORT_SLOTS * PORTS_PER_PROCESS;

    let slot = (std::process::id() % PORT_SLOTS as u32) as u16;
    let block_start = slot.wrapping_mul(PORTS_PER_PROCESS) % SPAN;

    // Walk the whole span rather than just this process's block: a test needing
    // more than PORTS_PER_PROCESS ports must still get them, it just borrows from
    // a neighbouring block and relies on the probe to skip anything in use.
    for _ in 0..SPAN {
        let step = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let port = PORT_BASE + (block_start.wrapping_add(step) % SPAN);
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
    panic!("no free port in {PORT_BASE}..{}", PORT_BASE + SPAN);
}

#[cfg(test)]
mod port_allocation_tests {
    use super::*;

    /// The property the CI failure was about: a port we hand out must be one the
    /// kernel will never hand to somebody else's `bind(:0)`.
    ///
    /// Linux's default ephemeral range starts at 32768; macOS's at 49152. Staying
    /// strictly below both is what makes the unavoidable close-then-bind window in
    /// `get_free_port` harmless.
    #[test]
    fn a_handed_out_port_is_never_one_the_os_can_auto_assign() {
        const LOWEST_EPHEMERAL_ANY_PLATFORM: u16 = 32_768;
        for _ in 0..32 {
            let port = get_free_port();
            assert!(
                (PORT_BASE..LOWEST_EPHEMERAL_ANY_PLATFORM).contains(&port),
                "port {port} is inside the OS ephemeral range, so a concurrent \
                 bind(:0) in another test process can be handed it too"
            );
        }
    }

    /// nextest spawns test processes with near-consecutive pids. Blocks are indexed
    /// by pid *multiplied* by the block size for exactly that reason: without the
    /// multiply, pid and pid+1 would start one port apart and overlap on their very
    /// first two allocations.
    #[test]
    fn neighbouring_pids_do_not_share_a_block() {
        let block = |pid: u32| (pid % PORT_SLOTS as u32) as u16 * PORTS_PER_PROCESS;
        for pid in 1000..1064u32 {
            let (mine, next) = (block(pid), block(pid + 1));
            assert!(
                mine.abs_diff(next) >= PORTS_PER_PROCESS,
                "pids {pid} and {} start {} ports apart; a test taking two ports \
                 would collide with its neighbour",
                pid + 1,
                mine.abs_diff(next)
            );
        }
    }

    /// A port is only returned if it actually binds, so the same port is never
    /// returned twice within a process while the caller still holds it.
    #[test]
    fn successive_calls_do_not_repeat_a_port() {
        let held: Vec<_> = (0..8)
            .map(|_| {
                let port = get_free_port();
                (
                    port,
                    TcpListener::bind(("127.0.0.1", port)).expect("probe said free"),
                )
            })
            .collect();
        let mut ports: Vec<u16> = held.iter().map(|(p, _)| *p).collect();
        let before = ports.len();
        ports.sort_unstable();
        ports.dedup();
        assert_eq!(
            before,
            ports.len(),
            "get_free_port handed out a duplicate: {ports:?}"
        );
    }
}
