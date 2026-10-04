//! The shell against doubles (`shell_rig`): each command reaches its port, and each answer
//! reaches the core.

use super::shell::{interest_until, Signal};
use super::shell_rig::*;
use super::types::Millis;
use super::types::{HealCause, LinkStatus, ProbeOutcome, SupervisorEvent};
use citadel_internal_service_types::P2pPathReport;
use std::sync::atomic::Ordering;
use std::time::Duration;

#[tokio::test]
async fn a_network_change_rebinds_then_probes_and_a_dead_path_ends_the_link() {
    let mut rig = rig(POLICY);
    rig.doubles
        .probes
        .lock()
        .extend([ProbeOutcome::Timeout, ProbeOutcome::Timeout]);
    rig.network.send(()).unwrap();
    assert_eq!(rig.next().await, Call::Rebind);
    assert_eq!(rig.next().await, Call::Probe);
    assert_eq!(rig.next().await, Call::Probe);
    assert_eq!(rig.next().await, Call::Force);
    assert_eq!(
        rig.next().await,
        Call::Report(SupervisorEvent::Healing {
            cause: HealCause::ProbesMissed
        })
    );
}

#[tokio::test]
async fn a_link_that_cannot_be_ended_is_probed_again_later() {
    let mut rig = rig(POLICY);
    rig.doubles.force_ends_link.store(false, Ordering::SeqCst);
    rig.doubles
        .probes
        .lock()
        .extend([ProbeOutcome::Timeout, ProbeOutcome::Timeout]);
    rig.network.send(()).unwrap();
    for expected in [Call::Rebind, Call::Probe, Call::Probe, Call::Force] {
        assert_eq!(rig.next().await, expected);
    }
    assert_eq!(
        rig.next().await,
        Call::Report(SupervisorEvent::Healing {
            cause: HealCause::ProbesMissed
        })
    );
    // When the failure is stamped does not matter: two intervals are past it either way.
    rig.advance(86_400_000);
    rig.advance(86_400_000);
    assert_eq!(rig.next().await, Call::Probe);
    assert_eq!(rig.next().await, Call::Report(SupervisorEvent::Healed));
}

#[tokio::test]
async fn a_queued_message_is_dialled_and_a_failure_backs_off() {
    let mut rig = rig(POLICY);
    rig.doubles.backlog.lock().insert(PEER, 2);
    rig.advance(2000);
    assert_eq!(
        rig.next().await,
        Call::Report(SupervisorEvent::PeerDialing { peer_cid: PEER })
    );
    assert_eq!(rig.next().await, Call::Dial(PEER));
    assert_eq!(
        rig.next().await,
        Call::Report(SupervisorEvent::Degraded { peer_cid: PEER })
    );
    rig.advance(999);
    rig.marker().await;
    rig.advance(1);
    assert_eq!(
        rig.next().await,
        Call::Dial(PEER),
        "1 s after the first failure"
    );
    rig.advance(1999);
    rig.marker().await;
    rig.advance(1);
    assert_eq!(rig.next().await, Call::Dial(PEER), "2 s after the second");
}

#[tokio::test]
async fn nothing_is_dialled_until_the_server_link_is_back() {
    let mut rig = rig(POLICY);
    rig.signals
        .send(Signal::Link(LinkStatus::Reconnecting))
        .unwrap();
    assert_eq!(
        rig.next().await,
        Call::Report(SupervisorEvent::Healing {
            cause: HealCause::LinkLost
        })
    );
    rig.signals
        .send(Signal::Interest {
            peer: PEER,
            ttl: Duration::from_secs(60),
        })
        .unwrap();
    rig.signals.send(Signal::Link(LinkStatus::Up)).unwrap();
    assert_eq!(rig.next().await, Call::Report(SupervisorEvent::Healed));
    assert_eq!(
        rig.next().await,
        Call::Report(SupervisorEvent::PeerDialing { peer_cid: PEER })
    );
    assert_eq!(rig.next().await, Call::Dial(PEER));
}

#[tokio::test]
async fn a_window_cannot_hold_a_peer_open_past_the_ceiling() {
    let mut rig = rig(POLICY);
    rig.signals
        .send(Signal::Interest {
            peer: PEER,
            ttl: Duration::from_secs(3600),
        })
        .unwrap();
    assert_eq!(
        rig.next().await,
        Call::Report(SupervisorEvent::PeerDialing { peer_cid: PEER })
    );
    assert_eq!(rig.next().await, Call::Dial(PEER));
    assert_eq!(
        rig.next().await,
        Call::Report(SupervisorEvent::Degraded { peer_cid: PEER })
    );
    rig.advance(121_000);
    rig.marker().await;
}

#[tokio::test]
async fn a_relayed_peer_a_window_has_open_is_upgraded() {
    let mut rig = rig(POLICY);
    rig.signals
        .send(Signal::PeerUp(PEER, P2pPathReport::ServerRelay))
        .unwrap();
    rig.signals
        .send(Signal::Interest {
            peer: PEER,
            ttl: Duration::from_secs(100),
        })
        .unwrap();
    rig.sync().await;
    rig.advance(5000);
    assert_eq!(rig.next().await, Call::Upgrade(PEER, true));
}

#[tokio::test]
async fn the_shell_ends_with_the_session() {
    let rig = rig(POLICY);
    rig.signals.send(Signal::Link(LinkStatus::Ended)).unwrap();
    tokio::time::timeout(Duration::from_secs(10), rig.task)
        .await
        .expect("the shell kept running")
        .expect("the shell did not panic");
}

#[tokio::test]
async fn a_probe_out_for_a_link_that_has_gone_says_nothing_about_the_new_one() {
    let mut rig = rig(POLICY);
    let (answer, gate) = tokio::sync::oneshot::channel();
    *rig.doubles.probe_gate.lock() = Some(gate);
    rig.network.send(()).unwrap();
    assert_eq!(rig.next().await, Call::Rebind);
    assert_eq!(rig.next().await, Call::Probe);
    rig.signals
        .send(Signal::Link(LinkStatus::Reconnecting))
        .unwrap();
    assert_eq!(
        rig.next().await,
        Call::Report(SupervisorEvent::Healing {
            cause: HealCause::LinkLost
        })
    );
    rig.signals.send(Signal::Link(LinkStatus::Up)).unwrap();
    assert_eq!(rig.next().await, Call::Report(SupervisorEvent::Healed));
    // The old probe times out now. Its answer must be gone with the link it was sent on, or
    // it would count as a miss and ask for another probe at once.
    let _ = answer.send(ProbeOutcome::Timeout);
    rig.marker().await;
}

#[test]
fn interest_ends_when_asked_but_never_past_the_ceiling() {
    let ceiling = Duration::from_secs(120);
    assert_eq!(
        interest_until(Millis(5), Duration::from_secs(30), ceiling),
        Millis(30_005)
    );
    assert_eq!(
        interest_until(Millis(5), Duration::from_secs(3600), ceiling),
        Millis(120_005)
    );
}
