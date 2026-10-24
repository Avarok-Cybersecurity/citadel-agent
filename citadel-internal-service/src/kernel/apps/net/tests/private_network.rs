//! `net.host.private` is the member's to allow, never the page's: the grant a page sends is
//! capped by the member's own grant on this agent (kernel/apps/settings.rs), which is unset
//! until they turn it on and is theirs alone.

use super::*;
use citadel_internal_service_types::AppFetchRefusal;

const BOB: u64 = 7;

async fn lan_fetch(settings: FakeSettings) -> (World, Result<(), AppFetchRefusal>) {
    let w = world_with(citadel_app_net_policy::AGENT_PORT, settings);
    w.resolver.answer("nas.lan", &["192.168.1.20"]);
    let fetched = w
        .fetch(
            &grant(true, &["nas.lan:5000"]),
            &get("http://nas.lan:5000/"),
        )
        .await
        .map(|_| ());
    (w, fetched)
}

#[tokio::test]
async fn a_private_fetch_is_refused_while_the_member_has_not_granted_it() {
    let (w, fetched) = lan_fetch(FakeSettings::default()).await;
    assert_eq!(fetched, Err(AppFetchRefusal::Policy));
    assert!(w.connector.attempts().is_empty(), "it connected");
}

#[tokio::test]
async fn another_members_grant_does_not_open_this_members_fetches() {
    let (w, fetched) = lan_fetch(FakeSettings::granting(BOB, true)).await;
    assert_eq!(fetched, Err(AppFetchRefusal::Policy));
    assert!(w.connector.attempts().is_empty(), "it connected");
}

#[tokio::test]
async fn the_same_fetch_is_allowed_once_this_member_grants_it() {
    // The control: the refusals above are the member's grant, not the request.
    let (w, fetched) = lan_fetch(FakeSettings::granting(key().cid, true)).await;
    assert_eq!(fetched, Ok(()));
    assert_eq!(
        w.connector.attempts()[0].connect,
        vec!["192.168.1.20:5000".parse().unwrap()]
    );
}

#[tokio::test]
async fn a_grant_turned_off_refuses_again() {
    let (w, fetched) = lan_fetch(FakeSettings::granting(key().cid, false)).await;
    assert_eq!(fetched, Err(AppFetchRefusal::Policy));
    assert!(w.connector.attempts().is_empty(), "it connected");
}

#[tokio::test]
async fn without_the_grant_public_hosts_are_still_served() {
    let w = world_with(citadel_app_net_policy::AGENT_PORT, FakeSettings::default());
    w.resolver.answer("api.acme.com", &["93.184.216.34"]);
    let fetched = w
        .fetch(
            &grant(true, &["api.acme.com"]),
            &get("https://api.acme.com/"),
        )
        .await;
    assert!(fetched.is_ok(), "{fetched:?}");
}
