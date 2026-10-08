//! The interleaving CI caught (run 37249572425): the supervisor abandons a dead path, the
//! link comes back under the same CID, the peer is being dialled, and only then does the
//! ABANDONED session's teardown arrive. The drop report goes through the decision the
//! disconnect handler makes (`reconnect::instance::on_reported_drop`), and its outcome is
//! fed to the core as `task::begin` and `task::spawn` feed it.

use super::super::types::{Command, HealCause, Input, LinkStatus, SupervisorEvent};
use super::tests::{core, secs, tick, timeout, PEER, TEST_POLICY};
use super::tests_peers::{dials, up};
use super::Core;
use crate::kernel::reconnect::instance::{on_reported_drop, Instance};
use crate::kernel::reconnect::policy::DropAction;
use crate::kernel::reconnect::LinkState;
use citadel_internal_service_types::P2pPathReport;
use citadel_sdk::prelude::Ticket;

const ABANDONED: Instance = Ticket(337_726_653);
const REPLACEMENT: Instance = Ticket(218_601_668);

/// The entry the disconnect handler consults: its link state and the instance it is.
struct Entry {
    link: LinkState,
    instance: Instance,
}

impl Entry {
    /// A drop report, handled as `responses/disconnect.rs` handles it: a reconnect begins
    /// only for a report about this link, and the supervisor hears of it.
    fn reported(&mut self, core: &mut Core, at: u64, reported: Option<Instance>) -> Vec<Command> {
        match on_reported_drop(self.link, self.instance, reported) {
            Some(DropAction::Reconnect) => {
                self.link = LinkState::Reconnecting;
                core.step(Input::ServerLink {
                    now: secs(at),
                    state: LinkStatus::Reconnecting,
                })
            }
            Some(DropAction::AlreadyReconnecting | DropAction::Remove) | None => Vec::new(),
        }
    }

    /// `reconnect/link.rs::put_link`: the new link and the instance it is, together.
    fn installed(&mut self, core: &mut Core, at: u64, instance: Instance) -> Vec<Command> {
        self.link = LinkState::Up;
        self.instance = instance;
        core.step(Input::ServerLink {
            now: secs(at),
            state: LinkStatus::Up,
        })
    }
}

fn healing(commands: &[Command]) -> bool {
    commands
        .iter()
        .any(|c| matches!(c, Command::Report(SupervisorEvent::Healing { .. })))
}

#[test]
fn the_abandoned_sessions_late_teardown_does_not_end_its_replacement() {
    let mut core = core(TEST_POLICY);
    let mut entry = Entry {
        link: LinkState::Up,
        instance: ABANDONED,
    };
    up(&mut core, 1, P2pPathReport::ServerRelay);

    // 1. The path dies silently: two probes go unanswered, and the link is abandoned.
    tick(&mut core, 15);
    timeout(&mut core, 20);
    let forced = timeout(&mut core, 25);
    assert!(forced.contains(&Command::ForceReconnect), "{forced:?}");
    assert!(forced.contains(&Command::Report(SupervisorEvent::Healing {
        cause: HealCause::ProbesMissed
    })));
    // `force::restart` begins the reconnect for the link it abandoned (it names none).
    assert!(
        entry.reported(&mut core, 26, None).is_empty(),
        "Healing was already said"
    );
    core.step(Input::PeerLost {
        now: secs(26),
        peer: PEER,
    });

    // 2. The link is back under the same CID, as a new SDK session, and the peer is dialled.
    let back = entry.installed(&mut core, 30, REPLACEMENT);
    assert!(
        back.contains(&Command::Report(SupervisorEvent::Healed)),
        "{back:?}"
    );
    assert_eq!(
        dials(&back),
        1,
        "the lost peer is dialled at once: {back:?}"
    );

    // 3. The abandoned session's own teardown arrives now, naming its own instance.
    let late = entry.reported(&mut core, 56, Some(ABANDONED));
    assert!(
        !healing(&late),
        "the replacement link was taken for the one that dropped: {late:?}"
    );
    assert_eq!(
        entry.link,
        LinkState::Up,
        "no reconnect begins for a healthy link"
    );

    // The replacement's own end is still a drop of this link.
    let own = entry.reported(&mut core, 60, Some(REPLACEMENT));
    assert!(healing(&own), "{own:?}");
    assert_eq!(entry.link, LinkState::Reconnecting);
}

#[test]
fn a_report_is_attributed_by_the_instance_it_names() {
    assert_eq!(
        on_reported_drop(LinkState::Up, REPLACEMENT, Some(ABANDONED)),
        None,
        "another instance's end"
    );
    assert_eq!(
        on_reported_drop(LinkState::Ending, REPLACEMENT, Some(ABANDONED)),
        None,
        "an older instance does not end a session its user is ending; its own report does"
    );
    assert_eq!(
        on_reported_drop(LinkState::Up, REPLACEMENT, Some(REPLACEMENT)),
        Some(DropAction::Reconnect)
    );
    assert_eq!(
        on_reported_drop(LinkState::Ending, REPLACEMENT, Some(REPLACEMENT)),
        Some(DropAction::Remove)
    );
    assert_eq!(
        on_reported_drop(LinkState::Up, REPLACEMENT, None),
        Some(DropAction::Reconnect),
        "a report naming no instance is the entry's, as before"
    );
}
