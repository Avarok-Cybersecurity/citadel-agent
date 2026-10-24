//! The workspace server decides what an install may reach; the page's grant only narrows it.
//! Each refusal here has its control beside it: the same fetch, the one fact changed, allowed.

use super::*;
use citadel_internal_service_types::{AppFetchRefusal, AppFetchUnavailable};
use std::sync::atomic::Ordering;

const API: &str = "https://api.acme.com/";
const EVIL: &str = "https://evil.example/";
const NAS: &str = "http://nas.lan:5000/";
const AUTHORITY: AppFetchRefusal = AppFetchRefusal::Unavailable(AppFetchUnavailable::Authority);

fn answering(w: &World) -> &World {
    w.resolver.answer("api.acme.com", &["93.184.216.34"]);
    w.resolver.answer("evil.example", &["93.184.216.99"]);
    w.resolver.answer("nas.lan", &["192.168.1.20"]);
    w
}

/// The server says the install reaches `api.acme.com` only, and no private address.
fn api_only(w: &World) {
    w.server.approve(
        &key().install_id,
        Some(server_grant(APP, &["api.acme.com"], false)),
    );
}

async fn fetched(w: &World, page: &AppNetGrant, url: &str) -> Result<(), AppFetchRefusal> {
    w.fetch(page, &get(url)).await.map(|_| ())
}

fn forged() -> AppNetGrant {
    grant(true, &["api.acme.com", "evil.example", "nas.lan:5000"])
}

#[tokio::test]
async fn a_host_the_page_adds_is_refused_before_anything_connects() {
    let w = world();
    api_only(answering(&w));
    assert_eq!(
        fetched(&w, &forged(), EVIL).await,
        Err(AppFetchRefusal::Policy)
    );
    assert!(w.connector.attempts().is_empty(), "it connected");
}

#[tokio::test]
async fn a_private_address_the_page_claims_is_refused() {
    let w = world();
    api_only(answering(&w));
    assert_eq!(
        fetched(&w, &forged(), NAS).await,
        Err(AppFetchRefusal::Policy)
    );
    assert!(w.connector.attempts().is_empty(), "it connected");
}

#[tokio::test]
async fn the_host_the_server_allows_is_fetched_through_the_same_forged_grant() {
    // The control for both above: the page's grant is the same, the host is the server's.
    let w = world();
    api_only(answering(&w));
    assert_eq!(fetched(&w, &forged(), API).await, Ok(()));
    let attempts = w.connector.attempts();
    assert_eq!(attempts.len(), 1);
    assert_eq!(
        attempts[0].connect,
        vec!["93.184.216.34:443".parse().unwrap()]
    );
}

#[tokio::test]
async fn a_page_naming_another_app_is_refused_and_the_install_s_own_app_is_not() {
    let w = world();
    answering(&w);
    let (install, page, request) = (key(), grant(false, &["api.acme.com"]), get(API));
    let other = w.net.fetch(
        w.input(&install, "com.evil.app", &page, &request),
        deadline(),
    );
    assert_eq!(other.await.map(|_| ()), Err(AppFetchRefusal::Policy));
    assert!(w.connector.attempts().is_empty(), "it connected");
    let own = w
        .net
        .fetch(w.input(&install, APP, &page, &request), deadline());
    assert_eq!(own.await.map(|_| ()), Ok(()));
}

#[tokio::test]
async fn the_servers_refusal_is_a_refusal() {
    let w = world();
    answering(&w);
    w.server.approve(&key().install_id, None);
    let page = grant(false, &["api.acme.com"]);
    assert_eq!(fetched(&w, &page, API).await, Err(AppFetchRefusal::Policy));
    assert!(w.connector.attempts().is_empty(), "it connected");
}

#[tokio::test]
async fn the_servers_grant_is_a_grant() {
    // The control: the refusal above is the server's answer, not the request.
    let w = world();
    api_only(answering(&w));
    let page = grant(false, &["api.acme.com"]);
    assert_eq!(fetched(&w, &page, API).await, Ok(()));
}

#[tokio::test(start_paused = true)]
async fn no_answer_from_the_server_is_no_fetch() {
    let w = world();
    answering(&w).server.silent.store(true, Ordering::SeqCst);
    let page = grant(false, &["api.acme.com"]);
    assert_eq!(fetched(&w, &page, API).await, Err(AUTHORITY));
    assert!(w.connector.attempts().is_empty(), "it connected");
    assert_eq!(w.server.questions(), 1, "it never asked");
}

#[tokio::test(start_paused = true)]
async fn an_answer_in_time_is_fetched() {
    // The control: the same clock and the same request, answered.
    let w = world();
    answering(&w);
    let page = grant(false, &["api.acme.com"]);
    assert_eq!(fetched(&w, &page, API).await, Ok(()));
}

#[tokio::test]
async fn a_fresh_answer_is_reused_until_the_members_installs_change() {
    let w = world();
    answering(&w);
    let page = grant(false, &["api.acme.com"]);
    assert_eq!(fetched(&w, &page, API).await, Ok(()));
    assert_eq!(fetched(&w, &page, API).await, Ok(()));
    assert_eq!(w.server.questions(), 1, "the second fetch asked again");
    // The control: once the server says the installs changed, the next fetch asks.
    let forwarded = w
        .net
        .grants
        .on_server_frame(key().cid, &FakeAuthority::installs_changed());
    assert!(forwarded, "the windows were not told the installs changed");
    assert_eq!(fetched(&w, &page, API).await, Ok(()));
    assert_eq!(w.server.questions(), 2);
}

#[tokio::test]
async fn nothing_is_served_from_memory_while_the_link_is_down() {
    let w = world();
    answering(&w);
    let page = grant(false, &["api.acme.com"]);
    assert_eq!(fetched(&w, &page, API).await, Ok(()));
    w.net.grants.link_down(key().cid);
    assert_eq!(fetched(&w, &page, API).await, Err(AUTHORITY));
    assert_eq!(
        w.connector.attempts().len(),
        1,
        "the refused fetch connected"
    );
    // Back up: the server is asked again rather than the old answer reused.
    w.net.grants.link_up(key().cid);
    assert_eq!(fetched(&w, &page, API).await, Ok(()));
    assert_eq!(w.server.questions(), 2);
}

#[tokio::test]
async fn with_the_link_up_the_answer_is_served_from_memory() {
    // The control for the link-down refusal: the same two fetches, no drop between them.
    let w = world();
    answering(&w);
    let page = grant(false, &["api.acme.com"]);
    assert_eq!(fetched(&w, &page, API).await, Ok(()));
    assert_eq!(fetched(&w, &page, API).await, Ok(()));
    assert_eq!(w.server.questions(), 1);
}

async fn lan(server_private: bool, member: FakeSettings) -> Result<(), AppFetchRefusal> {
    let w = world_with(citadel_app_net_policy::AGENT_PORT, member);
    answering(&w).server.approve(
        &key().install_id,
        Some(server_grant(APP, &["nas.lan:5000"], server_private)),
    );
    fetched(&w, &grant(true, &["nas.lan:5000"]), NAS).await
}

#[tokio::test]
async fn private_needs_the_server_and_the_member_both() {
    let alice = key().cid;
    assert_eq!(
        lan(true, FakeSettings::default()).await,
        Err(AppFetchRefusal::Policy),
        "the server allowed it, the member had not"
    );
    assert_eq!(
        lan(false, FakeSettings::granting(alice, true)).await,
        Err(AppFetchRefusal::Policy),
        "the member allowed it, the server did not"
    );
    // The control: both.
    assert_eq!(lan(true, FakeSettings::granting(alice, true)).await, Ok(()));
}
