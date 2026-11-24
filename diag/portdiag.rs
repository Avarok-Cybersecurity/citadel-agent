//! Throwaway diagnostic for WIN-PORTS-1123. Never merged.
use std::collections::BTreeMap;
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{Duration, Instant};

const PORT_BASE: u16 = 20_000;
const PORTS_PER_PROCESS: u16 = 16;
const PORT_SLOTS: u16 = 750;
const SPAN: u16 = PORT_SLOTS * PORTS_PER_PROCESS;

fn slot(pid: u32) -> u16 {
    (pid % PORT_SLOTS as u32) as u16
}

/// The helper as it is on master.
fn old_get_free_port() -> u16 {
    static NEXT: AtomicU16 = AtomicU16::new(0);
    let block_start = slot(std::process::id()).wrapping_mul(PORTS_PER_PROCESS) % SPAN;
    for _ in 0..SPAN {
        let step = NEXT.fetch_add(1, Ordering::Relaxed);
        let port = PORT_BASE + (block_start.wrapping_add(step) % SPAN);
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
    panic!("no free port");
}

fn child() {
    // What a test does: take two ports, then bind them, then hold them a while.
    let t0 = Instant::now();
    let a = old_get_free_port();
    let b = old_get_free_port();
    let pid = std::process::id();
    let mut held = Vec::new();
    for port in [a, b] {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(l) => held.push(l),
            Err(e) => {
                println!("CHILD pid={pid} slot={} port={port} BIND-FAILED {e:?} after {:?}", slot(pid), t0.elapsed());
                std::process::exit(3);
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(Duration::from_millis(1500));
    println!("CHILD pid={pid} slot={} ports={a},{b} ok", slot(pid));
}

fn close_then_rebind(rounds: u32) {
    let (mut probed, mut rebind_failed) = (0u32, 0u32);
    let mut codes: BTreeMap<String, u32> = BTreeMap::new();
    for i in 0..rounds {
        let port = PORT_BASE + (i % SPAN as u32) as u16;
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            probed += 1;
            if let Err(e) = TcpListener::bind(("127.0.0.1", port)) {
                rebind_failed += 1;
                *codes.entry(format!("{:?}", e.raw_os_error())).or_default() += 1;
            }
        } else {
            *codes.entry("probe-refused".into()).or_default() += 1;
        }
    }
    println!("H1 close-then-rebind, one process: probed_ok={probed} rebind_failed={rebind_failed} detail={codes:?}");
}

fn ephemeral_range() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (mut lo, mut hi, mut inside) = (u16::MAX, 0u16, 0u32);
    let mut listeners = vec![addr.port()];
    for _ in 0..3000 {
        let s = TcpStream::connect(addr).unwrap();
        let p = s.local_addr().unwrap().port();
        let _ = listener.accept().unwrap();
        lo = lo.min(p);
        hi = hi.max(p);
        inside += u32::from((PORT_BASE..PORT_BASE + SPAN).contains(&p));
        listeners.push(TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port());
    }
    println!("H2 ephemeral ports: connect() local ports {lo}..={hi}, {inside} of 3000 inside {PORT_BASE}..{}; bind(:0) ports {}..={}",
        PORT_BASE + SPAN, listeners.iter().min().unwrap(), listeners.iter().max().unwrap());
}

fn nextest_like(waves: u32, width: u32) {
    let me = std::env::current_exe().unwrap();
    let (mut failures, mut shared_slot_waves) = (0u32, 0u32);
    let mut all_pids = Vec::new();
    for wave in 0..waves {
        let kids: Vec<_> = (0..width).map(|_| Command::new(&me).arg("child").spawn().unwrap()).collect();
        let pids: Vec<u32> = kids.iter().map(|k| k.id()).collect();
        let mut slots: Vec<u16> = pids.iter().map(|p| slot(*p)).collect();
        slots.sort_unstable();
        let unique = { let mut s = slots.clone(); s.dedup(); s.len() };
        let shared = unique != slots.len();
        shared_slot_waves += u32::from(shared);
        let mut failed = 0;
        for mut k in kids {
            failed += u32::from(!k.wait().unwrap().success());
        }
        failures += failed;
        if shared || failed > 0 || wave < 5 {
            println!("WAVE {wave}: pids={pids:?} slots={slots:?} shared_slot={shared} failed={failed}");
        }
        all_pids.extend(pids);
    }
    let not_mult4 = all_pids.iter().filter(|p| *p % 4 != 0).count();
    println!("H3 nextest-like: waves={waves} width={width} waves_with_a_shared_slot={shared_slot_waves} child_failures={failures} pids_not_multiple_of_4={not_mult4} pid_range={:?}..={:?}",
        all_pids.iter().min(), all_pids.iter().max());
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("child") {
        return child();
    }
    println!("parent pid={}", std::process::id());
    close_then_rebind(60_000);
    ephemeral_range();
    nextest_like(150, 4);
}
