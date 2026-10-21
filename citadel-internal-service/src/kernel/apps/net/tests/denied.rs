//! Every denial is the one `Policy` refusal, and none of them reaches the connector.

use super::*;
use citadel_internal_service_types::AppFetchRefusal;

/// One address from every class the policy refuses without `private`.
const DENIED_WITHOUT_PRIVATE: [&str; 17] = [
    "127.0.0.1",       // loopback
    "::1",             // loopback v6
    "10.1.2.3",        // 10/8
    "172.16.0.9",      // 172.16/12
    "192.168.1.1",     // 192.168/16
    "169.254.10.10",   // link-local
    "fe80::1",         // link-local v6
    "fc00::5",         // unique local
    "100.64.0.1",      // shared / CGNAT
    "0.0.0.1",         // this network
    "192.0.0.8",       // IETF protocol assignments
    "198.18.0.1",      // benchmarking
    "224.0.0.251",     // multicast
    "::ffff:10.0.0.1", // mapped private
    "169.254.169.254", // metadata
    "255.255.255.255", // broadcast
    "192.0.2.10",      // documentation
];

/// Refused even under `net.host.private`.
const DENIED_EVEN_PRIVATE: [&str; 6] = [
    "169.254.169.254",
    "100.100.100.200",
    "fd00:ec2::254",
    "224.0.0.1",
    "0.0.0.0",
    "255.255.255.255",
];

async fn refused_without_connecting(w: &World, grant: &AppNetGrant, url: &str) {
    let before = w.connector.attempts().len();
    let result = w.fetch(grant, &get(url)).await;
    assert_eq!(result.map(|_| ()), Err(AppFetchRefusal::Policy), "{url}");
    assert_eq!(
        w.connector.attempts().len(),
        before,
        "{url}: a denied target reached the connector"
    );
}

#[tokio::test]
async fn no_denied_range_class_ever_reaches_the_connector() {
    for address in DENIED_WITHOUT_PRIVATE {
        let w = world();
        w.resolver.answer("api.acme.com", &[address]);
        refused_without_connecting(
            &w,
            &grant(false, &["api.acme.com"]),
            "https://api.acme.com/",
        )
        .await;
        // A public address beside it does not launder it: every address is decided.
        let w = world();
        w.resolver
            .answer("api.acme.com", &["93.184.216.34", address]);
        refused_without_connecting(
            &w,
            &grant(false, &["api.acme.com"]),
            "https://api.acme.com/",
        )
        .await;
    }
    for address in DENIED_EVEN_PRIVATE {
        let w = world();
        w.resolver.answer("intranet", &[address]);
        refused_without_connecting(&w, &grant(true, &["intranet"]), "https://intranet/").await;
    }
}

#[tokio::test]
async fn the_agent_itself_and_the_page_origin_are_refused_under_any_grant() {
    let w = world();
    w.resolver.answer("intranet", &["127.0.0.1"]);
    w.resolver.answer(PAGE, &["93.184.216.34"]);
    w.resolver.answer("local.avarok.net", &["127.0.0.1"]);
    let private = grant(true, &["intranet:12345", PAGE, "local.avarok.net"]);
    for url in [
        "http://intranet:12345/",
        "https://127.0.0.1:12345/",
        "https://acme.work.avarok.net/",
        "https://local.avarok.net/",
    ] {
        refused_without_connecting(&w, &private, url).await;
    }
    assert!(
        w.resolver.lookups.lock().is_empty(),
        "a hard-denied name was looked up"
    );
}

#[tokio::test]
async fn an_agent_bound_to_another_port_refuses_that_port_too() {
    let w = world_on_port(4000);
    w.resolver.answer("intranet", &["127.0.0.1"]);
    refused_without_connecting(
        &w,
        &grant(true, &["intranet:4000"]),
        "http://intranet:4000/",
    )
    .await;
    // The control: the same grant on another port is allowed.
    w.resolver.answer("intranet", &["127.0.0.1"]);
    let allowed = w
        .fetch(
            &grant(true, &["intranet:4001"]),
            &get("http://intranet:4001/"),
        )
        .await;
    assert!(allowed.is_ok(), "{allowed:?}");
}

#[tokio::test]
async fn a_name_the_grant_does_not_list_is_never_resolved() {
    let w = world();
    w.resolver.answer("evil.example", &["93.184.216.34"]);
    refused_without_connecting(
        &w,
        &grant(false, &["api.acme.com"]),
        "https://evil.example/",
    )
    .await;
    assert!(w.resolver.lookups.lock().is_empty());
}

#[tokio::test]
async fn requests_the_policy_reads_as_unsafe_are_refused_before_anything() {
    let w = world();
    w.resolver.answer("api.acme.com", &["93.184.216.34"]);
    let only = grant(false, &["api.acme.com"]);
    refused_without_connecting(&w, &only, "http://api.acme.com/").await;
    refused_without_connecting(&w, &only, "https://user:pw@api.acme.com/").await;
    refused_without_connecting(&w, &only, "https://api.acme.com:8080/").await;
    let mut smuggled = get("https://api.acme.com/");
    smuggled.headers.push(AppHttpHeader {
        name: "host".into(),
        value: "169.254.169.254".into(),
    });
    assert_eq!(
        w.fetch(&only, &smuggled).await.map(|_| ()),
        Err(AppFetchRefusal::Policy)
    );
    let mut get_with_body = get("https://api.acme.com/");
    get_with_body.body = b"x".to_vec();
    assert_eq!(
        w.fetch(&only, &get_with_body).await.map(|_| ()),
        Err(AppFetchRefusal::Policy)
    );
    let wildcard = grant(false, &["*.acme.com"]);
    refused_without_connecting(&w, &wildcard, "https://api.acme.com/").await;
    assert!(w.connector.attempts().is_empty());
}

#[tokio::test]
async fn a_rebinding_resolver_cannot_move_the_connection_off_the_checked_address() {
    let w = world();
    // Public when the agent checks, private for anyone who asks again.
    w.resolver.answer("api.acme.com", &["93.184.216.34"]);
    w.resolver.answer("api.acme.com", &["10.0.0.7"]);
    let fetched = w
        .fetch(
            &grant(false, &["api.acme.com"]),
            &get("https://api.acme.com/"),
        )
        .await;
    assert!(fetched.is_ok(), "{fetched:?}");
    let attempts = w.connector.attempts();
    assert_eq!(attempts.len(), 1);
    assert_eq!(
        attempts[0].connect,
        vec!["93.184.216.34:443".parse().unwrap()]
    );
    assert_eq!(
        w.resolver.lookups.lock().len(),
        1,
        "resolved more than once"
    );
}
