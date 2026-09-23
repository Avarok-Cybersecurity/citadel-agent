use super::{from_typed, validate, MAX_LEN};
use crate::kernel::server_address::classify;

fn typed(input: &str) -> Result<String, String> {
    from_typed(&classify(input).map_err(|err| format!("classify refused {input:?}: {err}"))?)
}

#[test]
fn a_typed_host_port_is_recorded_as_typed_but_trimmed_and_lowercased() {
    assert_eq!(
        typed(" Citadel.Avarok.net:12400 ").unwrap(),
        "citadel.avarok.net:12400"
    );
    assert_eq!(typed("localhost:12349").unwrap(), "localhost:12349");
}

#[test]
fn an_ip_address_is_recorded_as_the_ip_string() {
    assert_eq!(typed("127.0.0.1:12349").unwrap(), "127.0.0.1:12349");
    assert_eq!(typed("[::1]:12349").unwrap(), "[::1]:12349");
}

#[test]
fn a_hosted_tenant_is_recorded_as_its_host_without_the_implied_port() {
    assert_eq!(
        typed("acme.work.avarok.net").unwrap(),
        "acme.work.avarok.net"
    );
    assert_eq!(
        typed("wss://Acme.work.avarok.net/").unwrap(),
        "acme.work.avarok.net"
    );
}

#[test]
fn a_websocket_url_keeps_a_non_default_port_and_drops_its_path() {
    assert_eq!(typed("ws://localhost:8787/acme").unwrap(), "localhost:8787");
    assert_eq!(typed("ws://[::1]:8787/acme").unwrap(), "[::1]:8787");
    assert_eq!(typed("ws://example.com/").unwrap(), "example.com");
}

#[test]
fn valid_shapes_are_accepted() {
    for value in [
        "example.com",
        "example.com:1",
        "example.com:65535",
        "10.0.0.1",
        "[2001:db8::1]",
        "[2001:db8::1]:443",
        "a-b.c-d.example",
    ] {
        assert_eq!(validate(value), Ok(()), "{value:?}");
    }
}

#[test]
fn schemes_paths_userinfo_and_junk_are_refused() {
    for value in [
        "",
        "https://example.com",
        "example.com/path",
        "example.com:443/path",
        "user@example.com:443",
        "example.com:",
        "example.com:0",
        "example.com:65536",
        "example.com:+80",
        "example.com:80:80",
        "::1",
        "2001:db8::1:443",
        "[::1",
        "[::1]x",
        "[not-v6]:80",
        "exa mple.com",
        "-example.com",
        "example-.com",
        "example..com",
        "exämple.com",
        "example.com\n",
    ] {
        assert!(validate(value).is_err(), "{value:?} should be refused");
    }
}

#[test]
fn over_length_values_are_refused() {
    let label = "a".repeat(63);
    let name = [label.as_str(); 4].join(".");
    assert!(name.len() > 253);
    assert!(validate(&name).is_err(), "a DNS name over 253 bytes");
    assert!(validate(&"a".repeat(64)).is_err(), "a label over 63 bytes");
    assert!(validate(&"a.".repeat(MAX_LEN)).is_err(), "over MAX_LEN");
    // Every part of this is well formed; only its length is not.
    let padded_port = format!("example.com:{}80", "0".repeat(MAX_LEN));
    assert!(
        validate(&padded_port).is_err(),
        "over MAX_LEN by port padding"
    );
    let longest = format!(
        "{}:65535",
        [label.as_str(), &label, &label, &"a".repeat(61)].join(".")
    );
    assert_eq!(longest.len(), MAX_LEN);
    assert_eq!(validate(&longest), Ok(()));
}

#[test]
fn an_invalid_typed_address_is_refused_before_it_is_recorded() {
    assert!(typed("example.com/path").is_err());
    assert!(typed("user@example.com:443").is_err());
    assert!(typed(&format!("{}.com:1", "a".repeat(300))).is_err());
}
