//! The grant rules as a table (`citadel-session-abi.md` §1, §4).
//!
//! Controls, each applied, seen red and reverted:
//! - drop the `granted` check in `decide_open` → `a_server_outside_the_grant_is_ungranted` red.
//! - compare `server` uncanonicalised → `a_dialled_alias_of_a_granted_server_is_granted` red.
//! - drop the `APP_SESSION_MAX_BODY_BYTES` cap → `a_body_over_the_agents_cap_is_a_limit` red.

use super::*;

const APP: &str = "work.synchronized.app";

fn grant(servers: &[&str], session: Option<SessionLimits>) -> KernelGrant {
    KernelGrant {
        app_id: APP.into(),
        hosts: vec![],
        private: false,
        servers: servers.iter().map(|s| s.to_string()).collect(),
        session,
    }
}

const LIMITS: SessionLimits = SessionLimits {
    max_msg_bytes: 4096,
    max_msgs_per_s: 8,
};

type OpenRow<'a> = (
    Option<&'a KernelGrant>,
    &'a str,
    &'a str,
    Vec<Held>,
    Result<OpenDecision, Refusal>,
);
type SendRow<'a> = (
    Option<(&'a SessionStatus, u64, SessionLimits)>,
    usize,
    Result<u64, Refusal>,
);

fn held(server: &str, session: u32, status: SessionStatus) -> Held {
    Held {
        server: server.into(),
        session,
        status,
    }
}

fn opens(server: &str, replaces: Option<u32>) -> Result<OpenDecision, Refusal> {
    Ok(OpenDecision::Open {
        server: server.into(),
        limits: LIMITS,
        replaces,
    })
}

#[test]
fn a_server_outside_the_grant_is_ungranted() {
    let sw = grant(&["synchronized.work"], Some(LIMITS));
    let no_limits = grant(&["synchronized.work"], None);
    let no_servers = grant(&[], Some(LIMITS));
    let closed = SessionStatus::Closed {
        reason: "gone".into(),
    };
    let table: Vec<OpenRow> = vec![
        (
            Some(&sw),
            APP,
            "synchronized.work",
            vec![],
            opens("synchronized.work", None),
        ),
        (
            Some(&sw),
            APP,
            "evil.example",
            vec![],
            Err(Refusal::Ungranted),
        ),
        (
            Some(&sw),
            "com.other.app",
            "synchronized.work",
            vec![],
            Err(Refusal::Ungranted),
        ),
        (
            None,
            APP,
            "synchronized.work",
            vec![],
            Err(Refusal::Ungranted),
        ),
        (
            Some(&no_limits),
            APP,
            "synchronized.work",
            vec![],
            Err(Refusal::Ungranted),
        ),
        (
            Some(&no_servers),
            APP,
            "synchronized.work",
            vec![],
            Err(Refusal::Ungranted),
        ),
        (
            Some(&sw),
            APP,
            "not a name/../",
            vec![],
            Err(Refusal::Invalid),
        ),
        // A second open for the same server is the first session.
        (
            Some(&sw),
            APP,
            "synchronized.work",
            vec![held("synchronized.work", 7, SessionStatus::Connected)],
            Ok(OpenDecision::Reuse(7)),
        ),
        (
            Some(&sw),
            APP,
            "synchronized.work",
            vec![held("synchronized.work", 7, SessionStatus::Connecting)],
            Ok(OpenDecision::Reuse(7)),
        ),
        // A closed one is replaced, in its slot.
        (
            Some(&sw),
            APP,
            "synchronized.work",
            vec![held("synchronized.work", 7, closed.clone())],
            opens("synchronized.work", Some(7)),
        ),
    ];
    for (i, (grant, app, server, held, want)) in table.into_iter().enumerate() {
        assert_eq!(
            decide_open(grant, app, server, &held),
            want,
            "row {i}: {server}"
        );
    }
}

#[test]
fn a_dialled_alias_of_a_granted_server_is_granted() {
    let sw = grant(&["synchronized.work"], Some(LIMITS));
    assert_eq!(
        decide_open(Some(&sw), APP, "https://Synchronized.Work/", &[]),
        opens("synchronized.work", None)
    );
}

#[test]
fn a_third_server_per_install_is_a_limit() {
    let three = grant(&["a.example", "b.example", "c.example"], Some(LIMITS));
    let two = vec![
        held("a.example", 1, SessionStatus::Connected),
        held("b.example", 2, SessionStatus::Connecting),
    ];
    assert_eq!(
        decide_open(Some(&three), APP, "c.example", &two),
        Err(Refusal::Limit)
    );
    let one_closed = vec![
        held("a.example", 1, SessionStatus::Connected),
        held("b.example", 2, SessionStatus::Closed { reason: "x".into() }),
    ];
    assert_eq!(
        decide_open(Some(&three), APP, "c.example", &one_closed),
        opens("c.example", None)
    );
}

#[test]
fn a_send_needs_a_connected_session_within_its_limit() {
    let closed = SessionStatus::Closed { reason: "x".into() };
    let table: Vec<SendRow> = vec![
        (Some((&SessionStatus::Connected, 42, LIMITS)), 4096, Ok(42)),
        (
            Some((&SessionStatus::Connected, 42, LIMITS)),
            4097,
            Err(Refusal::Limit),
        ),
        (
            Some((&SessionStatus::Connecting, 0, LIMITS)),
            1,
            Err(Refusal::Invalid),
        ),
        (Some((&closed, 42, LIMITS)), 1, Err(Refusal::Invalid)),
        (None, 1, Err(Refusal::Invalid)),
    ];
    for (i, (session, len, want)) in table.into_iter().enumerate() {
        assert_eq!(decide_send(session, len), want, "row {i}");
    }
}

#[test]
fn a_body_over_the_agents_cap_is_a_limit() {
    let generous = SessionLimits {
        max_msg_bytes: u32::MAX,
        max_msgs_per_s: 1,
    };
    let at_cap = APP_SESSION_MAX_BODY_BYTES;
    let connected = Some((&SessionStatus::Connected, 9, generous));
    assert_eq!(decide_send(connected, at_cap), Ok(9));
    assert_eq!(decide_send(connected, at_cap + 1), Err(Refusal::Limit));
}

#[test]
fn the_handle_is_the_fingerprint_and_the_install_prefix() {
    assert_eq!(
        app_handle("abcdefghijklmnop", "0f3a9c2e-1111-2222").as_deref(),
        Some("abcdefghijklmnop.app-0f3a9c2e")
    );
    assert_eq!(app_handle("abcdefghijklmnop", "short"), None);
    assert_eq!(
        app_handle("abcdefghijklmnop", "ABCDEFGH"),
        None,
        "a handle is lowercase"
    );
}

#[test]
fn every_refusal_has_its_wire_name() {
    let names: Vec<&str> = [
        Refusal::Ungranted,
        Refusal::Limit,
        Refusal::Invalid,
        Refusal::NoIdentity,
        Refusal::IdentityLocked,
        Refusal::NotConfigured,
        Refusal::Session,
        Refusal::Authority,
        Refusal::Unavailable("x".into()),
    ]
    .iter()
    .map(Refusal::wire)
    .collect();
    assert_eq!(names[0], "ungranted");
    assert_eq!(names[3], "no_identity");
    let unique: std::collections::HashSet<_> = names.iter().collect();
    assert_eq!(unique.len(), names.len());
}
