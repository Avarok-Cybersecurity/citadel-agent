use super::*;
use citadel_sdk::prefabs::server::empty::EmptyKernel;
use citadel_sdk::prelude::{BackendType, NodeType, StackedRatchet};

const THREE: &str = "stun.cloudflare.com:3478,127.0.0.1:3479,[::1]:3480";

fn servers(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn nothing_configured_is_a_startup_error_naming_the_flag_and_variable() {
    let message = StunServers::resolve(None, None).unwrap_err();
    assert!(message.contains(STUN_SERVERS_FLAG), "{message}");
    assert!(message.contains(STUN_SERVERS_ENV), "{message}");
}

#[test]
fn empty_strings_count_as_unset() {
    assert!(StunServers::resolve(Some(""), Some("")).is_err());
}

#[test]
fn the_cli_flag_is_used_when_the_env_var_is_unset() {
    let parsed = StunServers::resolve(None, Some(THREE)).unwrap();
    assert_eq!(
        parsed.as_slice(),
        servers(&["stun.cloudflare.com:3478", "127.0.0.1:3479", "[::1]:3480"])
    );
}

#[test]
fn the_env_var_wins_over_the_cli_flag() {
    let env = "a.example:1,b.example:2,c.example:3";
    let parsed = StunServers::resolve(Some(env), Some(THREE)).unwrap();
    assert_eq!(
        parsed.as_slice(),
        servers(&["a.example:1", "b.example:2", "c.example:3"])
    );
}

#[test]
fn an_empty_env_var_falls_through_to_the_cli_flag() {
    let parsed = StunServers::resolve(Some(""), Some(THREE)).unwrap();
    assert_eq!(parsed, StunServers::parse(THREE).unwrap());
}

#[test]
fn entries_are_trimmed() {
    let parsed = StunServers::parse(" a.example:1 , b.example:2,c.example:3 ").unwrap();
    assert_eq!(
        parsed.as_slice(),
        servers(&["a.example:1", "b.example:2", "c.example:3"])
    );
}

#[test]
fn a_malformed_list_fails_startup_naming_the_flag() {
    let message = StunServers::resolve(None, Some("stun.cloudflare.com")).unwrap_err();
    assert!(message.contains(STUN_SERVERS_FLAG), "{message}");
}

#[test]
fn the_count_must_be_exactly_what_the_sdk_accepts() {
    assert!(StunServers::parse("a.example:1").is_err());
    assert!(StunServers::parse("a.example:1,b.example:2").is_err());
    assert!(StunServers::parse("a.example:1,b.example:2,c.example:3,d.example:4").is_err());
}

#[test]
fn schemes_are_rejected() {
    for bad in [
        "stun:stun.cloudflare.com:3478",
        "STUN:stun.cloudflare.com:3478",
        "stuns:stun.cloudflare.com:5349",
        "turn:stun.cloudflare.com:3478",
        "udp://stun.cloudflare.com:3478",
        "https://stun.cloudflare.com:3478",
    ] {
        let spec = format!("{bad},b.example:2,c.example:3");
        let message = StunServers::parse(&spec).unwrap_err();
        assert!(message.contains("scheme"), "{bad}: {message}");
    }
}

#[test]
fn entries_that_are_not_host_port_are_rejected() {
    for bad in [
        "",
        "stun.cloudflare.com",
        "stun.cloudflare.com:",
        "stun.cloudflare.com:0",
        "stun.cloudflare.com:65536",
        "stun.cloudflare.com:port",
        ":3478",
        "user@stun.cloudflare.com:3478",
        "stun.cloudflare.com:3478/path",
        "stun .cloudflare.com:3478",
        "-bad.example:3478",
        "bad..example:3478",
        "::1:3478",
        "[not-ipv6]:3478",
    ] {
        let spec = format!("{bad},b.example:2,c.example:3");
        assert!(StunServers::parse(&spec).is_err(), "accepted {bad:?}");
    }
}

#[test]
fn an_overlong_entry_is_rejected() {
    // The host and port checks alone accept this: u16 parsing allows leading
    // zeros. Only the entry-length bound refuses it.
    let port = format!("{}3478", "0".repeat(MAX_ENTRY_LEN));
    let spec = format!("a.example:{port},b.example:2,c.example:3");
    assert!(StunServers::parse(&spec).is_err());
}

#[derive(Default)]
struct RecordingSink(Option<Vec<String>>);

impl StunServerSink for RecordingSink {
    fn set_stun_servers(&mut self, servers: Vec<String>) {
        self.0 = Some(servers);
    }
}

#[test]
fn apply_delivers_the_configured_list_in_order() {
    let parsed = StunServers::parse(THREE).unwrap();
    let mut sink = RecordingSink::default();
    parsed.apply(&mut sink);
    assert_eq!(sink.0.as_deref(), Some(parsed.as_slice()));
}

fn peer_builder() -> NodeBuilder<StackedRatchet> {
    let mut builder = NodeBuilder::<StackedRatchet>::default();
    builder
        .with_backend(BackendType::InMemory)
        .with_node_type(NodeType::Peer);
    builder
}

#[test]
fn the_sdk_builder_accepts_a_parsed_list() {
    let mut builder = peer_builder();
    StunServers::parse(THREE).unwrap().apply(&mut builder);
    assert!(builder
        .build(EmptyKernel::<StackedRatchet>::default())
        .is_ok());
}

#[test]
fn the_sdk_builder_receives_the_list_through_the_sink() {
    // The SDK has no getter, but it rejects any count but three at build().
    // Delivering a wrong-count list through the sink and seeing build() fail
    // proves the sink reaches the SDK's own field rather than a no-op.
    let mut builder = peer_builder();
    builder.set_stun_servers(servers(&["only.example:1"]));
    assert!(builder
        .build(EmptyKernel::<StackedRatchet>::default())
        .is_err());
}
