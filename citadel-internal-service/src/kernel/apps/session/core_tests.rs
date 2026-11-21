//! A request against the registry, through a recording transport.
//!
//! Why a recording double, not the SDK: what is under test is that a refused open never
//! reaches the agent's session machinery at all. The double records every call made to the
//! port and answers each one as the real machinery's success would; the property is read off
//! what it recorded. The real machinery is driven end to end by
//! tests/an_app_session_reconnects_under_its_cid.rs.
//!
//! Control (the named one): make `rules::decide_open`'s `granted` always true (grant all
//! servers) → the ungranted open reaches the transport and
//! `an_ungranted_server_gets_no_agent_call` goes red.

use super::core::{self, AppAccount, SessionTransport};
use super::rules::Refusal;
use super::{AppSessions, Owner};
use crate::kernel::apps::net::authority::{KernelGrant, SessionLimits};
use crate::kernel::apps::net::grants::GrantFault;
use citadel_internal_service_types::SessionStatus;
use parking_lot::Mutex;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    Open(AppAccount),
    Send(u64, usize),
    Close(u64),
}

#[derive(Default)]
struct Recording {
    calls: Mutex<Vec<Call>>,
}

const SERVER_CID: u64 = 4242;

#[async_trait::async_trait]
impl SessionTransport for Recording {
    async fn open(&self, account: &AppAccount) -> Result<u64, Refusal> {
        self.calls.lock().push(Call::Open(account.clone()));
        Ok(SERVER_CID)
    }
    async fn send(&self, server_cid: u64, body: Vec<u8>) -> Result<(), Refusal> {
        self.calls.lock().push(Call::Send(server_cid, body.len()));
        Ok(())
    }
    async fn close(&self, server_cid: u64) -> Result<(), Refusal> {
        self.calls.lock().push(Call::Close(server_cid));
        Ok(())
    }
}

const APP: &str = "work.synchronized.app";

fn granted(servers: &[&str]) -> Result<Option<KernelGrant>, GrantFault> {
    Ok(Some(KernelGrant {
        app_id: APP.into(),
        hosts: vec![],
        private: false,
        servers: servers.iter().map(|s| s.to_string()).collect(),
        session: Some(SessionLimits {
            max_msg_bytes: 64,
            max_msgs_per_s: 4,
        }),
    }))
}

fn owner() -> Owner {
    Owner {
        member_cid: 7,
        install_id: "0f3a9c2e-aaaa".into(),
        acting_username: "alice@main.work.avarok.net".into(),
    }
}

#[tokio::test]
async fn an_ungranted_server_gets_no_agent_call() {
    let (sessions, transport) = (AppSessions::default(), Recording::default());
    let refused = core::open(
        &sessions,
        &transport,
        granted(&["synchronized.work"]),
        APP,
        owner(),
        "evil.example",
    )
    .await;
    assert_eq!(refused, Err(Refusal::Ungranted));
    assert_eq!(*transport.calls.lock(), vec![], "nothing reached the agent");

    // The server saying nothing, or not answering, is no call either.
    for grant in [Ok(None), Err(GrantFault::Timeout)] {
        let answer = core::open(
            &sessions,
            &transport,
            grant,
            APP,
            owner(),
            "synchronized.work",
        );
        assert!(answer.await.is_err());
    }
    assert_eq!(*transport.calls.lock(), vec![]);
}

#[tokio::test]
async fn a_granted_server_is_opened_once_and_reused() {
    let (sessions, transport) = (AppSessions::default(), Recording::default());
    let open = || {
        core::open(
            &sessions,
            &transport,
            granted(&["synchronized.work"]),
            APP,
            owner(),
            "synchronized.work",
        )
    };
    let (session, cid, state) = open().await.unwrap();
    assert_eq!((cid, state), (SERVER_CID, SessionStatus::Connected));
    assert_eq!(
        open().await.unwrap(),
        (session, SERVER_CID, SessionStatus::Connected)
    );
    let calls = transport.calls.lock().clone();
    assert_eq!(
        calls,
        vec![Call::Open(AppAccount {
            owner: owner(),
            session,
            server: "synchronized.work".into(),
        })],
        "one account sign-in for both opens"
    );
    assert!(sessions.holds_server_cid(SERVER_CID));
}

#[tokio::test]
async fn sends_and_closes_are_only_the_installs_own() {
    let (sessions, transport) = (AppSessions::default(), Recording::default());
    let grant = granted(&["synchronized.work"]);
    let (session, ..) = core::open(
        &sessions,
        &transport,
        grant,
        APP,
        owner(),
        "synchronized.work",
    )
    .await
    .unwrap();
    let install = owner().install_id;
    let other = "ffffffff-other";
    assert_eq!(
        core::send(&sessions, &transport, 7, other, session, vec![1]).await,
        Err(Refusal::Invalid)
    );
    assert_eq!(
        core::send(&sessions, &transport, 8, &install, session, vec![1]).await,
        Err(Refusal::Invalid),
        "another member's session"
    );
    assert_eq!(
        core::send(&sessions, &transport, 7, &install, session, vec![0; 65]).await,
        Err(Refusal::Limit)
    );
    core::send(&sessions, &transport, 7, &install, session, vec![0; 64])
        .await
        .unwrap();
    assert_eq!(
        core::close(&sessions, &transport, 7, other, session).await,
        Err(Refusal::Invalid)
    );
    core::close(&sessions, &transport, 7, &install, session)
        .await
        .unwrap();
    assert_eq!(
        core::send(&sessions, &transport, 7, &install, session, vec![1]).await,
        Err(Refusal::Invalid),
        "closed"
    );
    let calls = transport.calls.lock().clone();
    assert_eq!(
        calls[1..],
        [Call::Send(SERVER_CID, 64), Call::Close(SERVER_CID)]
    );
    assert!(!sessions.holds_server_cid(SERVER_CID));
}

#[tokio::test]
async fn a_failed_sign_in_frees_the_slot() {
    struct Down;
    #[async_trait::async_trait]
    impl SessionTransport for Down {
        async fn open(&self, _: &AppAccount) -> Result<u64, Refusal> {
            Err(Refusal::NoIdentity)
        }
        async fn send(&self, _: u64, _: Vec<u8>) -> Result<(), Refusal> {
            unreachable!()
        }
        async fn close(&self, _: u64) -> Result<(), Refusal> {
            unreachable!()
        }
    }
    let sessions = AppSessions::default();
    let grant = || granted(&["synchronized.work"]);
    let first = core::open(&sessions, &Down, grant(), APP, owner(), "synchronized.work").await;
    assert_eq!(first, Err(Refusal::NoIdentity));
    let recording = Recording::default();
    let again = core::open(
        &sessions,
        &recording,
        grant(),
        APP,
        owner(),
        "synchronized.work",
    );
    assert!(
        again.await.is_ok(),
        "the failed open left no session to reuse"
    );
    assert_eq!(recording.calls.lock().len(), 1);
}
