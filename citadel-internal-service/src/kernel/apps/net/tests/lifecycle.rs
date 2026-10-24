//! The service's own teardown paths reach the grant cache: a dropped link refuses from memory
//! (cid_scoped_state.rs `prune_dropped_link_state`), and an ended session forgets the account.

use super::super::grants::GrantFault;
use super::*;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::in_memory::InMemoryInterface;
use citadel_sdk::prelude::StackedRatchet;

type Svc = CitadelWorkspaceService<InMemoryInterface, StackedRatchet>;

fn service() -> (tempfile::TempDir, Svc) {
    let transfers = tempfile::tempdir().expect("temp dir");
    let (_connector, svc): (_, Svc) = CitadelWorkspaceService::new_in_memory(
        crate::SERVER_RECONNECT,
        crate::BrowserTransferRoot::at(transfers.path().to_path_buf()),
    );
    let config = AppNetConfig {
        page_origin_hosts: vec![PAGE.to_string()],
        agent_port: citadel_app_net_policy::AGENT_PORT,
    };
    (transfers, svc.with_app_net(config, Arc::new(FakeAuthority)))
}

async fn ask(svc: &Svc, server: &FakeServer) -> Result<bool, GrantFault> {
    let net = svc.apps.net.as_ref().expect("host egress configured");
    let grant = net
        .grants
        .resolve(key().cid, &key().install_id, server)
        .await?;
    Ok(grant.is_some())
}

fn server_for(svc: &Svc) -> FakeServer {
    let net = svc.apps.net.as_ref().expect("host egress configured");
    FakeServer::new(
        net.grants.clone(),
        server_grant(APP, &["api.acme.com"], false),
    )
}

#[tokio::test]
async fn a_dropped_link_refuses_until_it_is_back() {
    let (_dir, svc) = service();
    let server = server_for(&svc);
    assert_eq!(ask(&svc, &server).await, Ok(true));
    let _ = svc.prune_dropped_link_state(key().cid);
    assert_eq!(ask(&svc, &server).await, Err(GrantFault::LinkDown));
    assert_eq!(server.questions(), 1, "a down link was asked");
}

#[tokio::test]
async fn an_ended_session_is_forgotten_and_asked_afresh() {
    // The control for the refusal above: a teardown that ends the session does not leave it
    // refused (the next sign-in's link is new), but it does drop the old answer.
    let (_dir, svc) = service();
    let server = server_for(&svc);
    assert_eq!(ask(&svc, &server).await, Ok(true));
    let _ = svc.prune_dropped_link_state(key().cid);
    let _ = svc.prune_cid_scoped_state(key().cid, None);
    assert_eq!(ask(&svc, &server).await, Ok(true));
    assert_eq!(server.questions(), 2);
}

#[tokio::test]
async fn a_peer_teardown_keeps_the_answer() {
    let (_dir, svc) = service();
    let server = server_for(&svc);
    assert_eq!(ask(&svc, &server).await, Ok(true));
    let _ = svc.prune_cid_scoped_state(key().cid, Some(7));
    assert_eq!(ask(&svc, &server).await, Ok(true));
    assert_eq!(
        server.questions(),
        1,
        "a P2P teardown dropped the server's answer"
    );
}
