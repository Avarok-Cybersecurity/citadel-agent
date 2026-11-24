//! Control: in `claim`, answer `WouldBlock` with `Some(file)`, so a claim excludes nobody
//! and only the pid arithmetic is left, as it was. The two tests below about two holders
//! then fail, each naming the ports handed out twice.

use super::*;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

/// Two pids a Windows CI runner gave to test processes alive at the same time. They are
/// 3000 apart, so `pid % 750` is 152 for both.
const ONE_LIVE_PID: u32 = 4652;
const ANOTHER_LIVE_PID: u32 = 7652;
const HANDED_OUT: &str = "handed out:";

fn two_ports(as_pid: u32) -> (PortAllocator, [u16; 2]) {
    let mut ports = PortAllocator::for_pid(as_pid);
    let handed = [ports.next_port(), ports.next_port()];
    (ports, handed)
}

/// Staying strictly below every platform's ephemeral range is what keeps the OS from
/// giving a port of ours to a `bind(:0)` or an outgoing connection.
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

/// More than one block's worth, none of them bound: nothing but the allocator itself
/// keeps a port from being handed out again.
#[test]
fn successive_calls_do_not_repeat_a_port() {
    let mut ports: Vec<u16> = (0..3 * PORTS_PER_BLOCK).map(|_| get_free_port()).collect();
    let handed = ports.len();
    ports.sort_unstable();
    ports.dedup();
    assert_eq!(
        handed,
        ports.len(),
        "a port was handed out twice: {ports:?}"
    );
}

/// A block is a whole claimed block, none of whose ports a single-port request is handed.
#[test]
fn a_block_shares_no_port_with_single_ports() {
    let singles: Vec<u16> = (0..PORTS_PER_BLOCK).map(|_| get_free_port()).collect();
    let block = get_free_port_block();
    assert_eq!(block.len(), usize::from(PORTS_PER_BLOCK));
    assert!(
        block.start >= PORT_BASE && block.end <= PORT_END,
        "{block:?}"
    );
    let later: Vec<u16> = (0..PORTS_PER_BLOCK).map(|_| get_free_port()).collect();
    let shared: Vec<&u16> = singles
        .iter()
        .chain(&later)
        .filter(|p| block.contains(p))
        .collect();
    assert!(
        shared.is_empty(),
        "{block:?} shares {shared:?} with single ports"
    );
}

/// The test's process, standing in for another test: it takes two ports, binds neither,
/// and stays alive until its stdin closes. Run only by the test below.
#[test]
#[ignore = "a helper process for a_port_handed_to_a_live_process_is_not_handed_to_another"]
fn poses_as_a_test_process_that_has_not_bound_its_ports_yet() {
    let (_claims, [first, second]) = two_ports(ANOTHER_LIVE_PID);
    println!("{HANDED_OUT} {first} {second}");
    let mut until_closed = String::new();
    std::io::stdin().read_line(&mut until_closed).unwrap();
}

/// The CI failure: two live processes whose pids share a block, each handed ports it has
/// not bound yet. Neither may be handed a port the other was.
#[test]
fn a_port_handed_to_a_live_process_is_not_handed_to_another() {
    let mut other = Command::new(std::env::current_exe().unwrap())
        .args(["poses_as_a_test_process_that_has_not_bound_its_ports_yet"])
        .args(["--ignored", "--nocapture", "--test-threads=1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    // Kept open to the end: the harness in the other process still has its verdict to print.
    let mut said = BufReader::new(other.stdout.take().unwrap()).lines();
    let theirs: Vec<u16> = said
        .by_ref()
        .map(|line| line.unwrap())
        .find_map(|line| {
            let ports = line.split(HANDED_OUT).nth(1)?;
            Some(
                ports
                    .split_whitespace()
                    .map(|port| port.parse().unwrap())
                    .collect(),
            )
        })
        .expect("the other process says which ports it was handed");
    assert_eq!(theirs.len(), 2, "the other process was handed two ports");

    let (_claims, mine) = two_ports(ONE_LIVE_PID);

    drop(other.stdin.take());
    let rest: Vec<String> = said.map(|line| line.unwrap()).collect();
    assert!(
        other.wait().unwrap().success(),
        "the other process failed: {rest:?}"
    );
    assert!(
        mine.iter().all(|port| !theirs.contains(port)),
        "two live processes were both handed a port: {mine:?} and {theirs:?}"
    );
}

/// The same, with both holders in this process: a claim excludes whoever else asks, not
/// only another process.
#[test]
fn two_holders_whose_pids_share_a_block_are_handed_different_ports() {
    let (_one, first) = two_ports(ONE_LIVE_PID);
    let (_another, second) = two_ports(ANOTHER_LIVE_PID);
    assert!(
        first.iter().all(|port| !second.contains(port)),
        "both were handed a port: {first:?} and {second:?}"
    );
}
