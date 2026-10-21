//! What an allowed request is, and an agent that cannot allow any.

use super::*;

#[tokio::test]
async fn an_allowed_request_is_one_attempt_pinned_to_the_checked_address_with_nothing_added() {
    let w = world();
    w.resolver.answer("api.acme.com", &["93.184.216.34"]);
    let request = get("https://api.acme.com/v1/items?x=1");
    let fetched = w
        .fetch(&grant(false, &["api.acme.com"]), &request)
        .await
        .expect("allowed");
    assert_eq!(fetched.status, 200);
    assert_eq!(fetched.body, b"ok");
    let attempts = w.connector.attempts();
    assert_eq!(attempts.len(), 1);
    let attempt = &attempts[0];
    assert_eq!(attempt.connect, vec!["93.184.216.34:443".parse().unwrap()]);
    assert_eq!(attempt.host, "api.acme.com");
    assert_eq!(attempt.url, "https://api.acme.com:443/v1/items?x=1");
    assert_eq!(
        attempt.headers,
        vec![("accept".to_string(), "application/json".to_string())],
        "the runtime added or changed a header"
    );
    let usage = w.net.meter.read(&key());
    assert_eq!((usage.requests, usage.bytes_in, usage.refused), (1, 2, 0));
}

#[tokio::test]
async fn an_agent_with_no_page_origin_refuses_to_configure_host_egress() {
    let ports = NetPorts {
        resolver: Box::new(Arc::new(FakeResolver::default())),
        connector: Box::new(Arc::new(FakeConnector::default())),
        clock: Box::new(Arc::new(FakeClock::default())),
    };
    let config = AppNetConfig {
        page_origin_hosts: vec![],
        agent_port: citadel_app_net_policy::AGENT_PORT,
    };
    assert!(AppNet::new(config, ports).is_none());
}
