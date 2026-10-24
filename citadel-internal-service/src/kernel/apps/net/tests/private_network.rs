//! `net.host.private` is the agent's to allow, never the page's: the grant a page sends is
//! capped by the agent's own `private_network`.

use super::*;
use citadel_internal_service_types::AppFetchRefusal;

async fn lan_fetch(private_network: bool) -> (World, Result<(), AppFetchRefusal>) {
    let w = world_with(citadel_app_net_policy::AGENT_PORT, private_network);
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
async fn a_private_fetch_is_refused_by_an_agent_that_keeps_the_private_network_closed() {
    let (w, fetched) = lan_fetch(false).await;
    assert_eq!(fetched, Err(AppFetchRefusal::Policy));
    assert!(w.connector.attempts().is_empty(), "it connected");
}

#[tokio::test]
async fn the_same_fetch_is_allowed_by_an_agent_that_opens_it() {
    // The control: the refusal above is the agent's setting, not the request.
    let (w, fetched) = lan_fetch(true).await;
    assert_eq!(fetched, Ok(()));
    assert_eq!(
        w.connector.attempts()[0].connect,
        vec!["192.168.1.20:5000".parse().unwrap()]
    );
}

#[tokio::test]
async fn a_closed_private_network_still_serves_public_hosts() {
    let w = world_with(citadel_app_net_policy::AGENT_PORT, false);
    w.resolver.answer("api.acme.com", &["93.184.216.34"]);
    let fetched = w
        .fetch(
            &grant(true, &["api.acme.com"]),
            &get("https://api.acme.com/"),
        )
        .await;
    assert!(fetched.is_ok(), "{fetched:?}");
}
