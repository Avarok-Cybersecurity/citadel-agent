//! A grant's host list: explicit names only, at most eight, no wildcards.

use super::*;

#[test]
fn up_to_eight_explicit_hosts_are_a_grant() {
    let eight: Vec<String> = (0..MAX_HOSTS).map(|i| format!("h{i}.acme.com")).collect();
    let eight: Vec<&str> = eight.iter().map(String::as_str).collect();
    let grant = NetGrant::new(Egress::Server, &eight).unwrap();
    assert_eq!(grant.hosts().len(), MAX_HOSTS);
    let nine = [eight.as_slice(), &["h8.acme.com"]].concat();
    assert_eq!(
        NetGrant::new(Egress::Server, &nine),
        Err(GrantFault::TooManyHosts)
    );
    assert_eq!(NetGrant::new(Egress::Server, &[]), Err(GrantFault::NoHosts));
}

#[test]
fn a_wildcard_anywhere_is_refused() {
    for entry in ["*", "*.acme.com", "api.*.com", "api.acme.*", "*:443"] {
        assert_eq!(
            AllowedHost::parse(entry),
            Err(GrantFault::Wildcard(entry.to_string())),
            "{entry}"
        );
    }
}

#[test]
fn an_entry_is_a_host_and_optional_port_only() {
    assert_eq!(
        AllowedHost::parse("api.acme.com:8080"),
        Ok(AllowedHost {
            host: "api.acme.com".to_string(),
            port: Some(8080)
        })
    );
    for entry in [
        "https://api.acme.com",
        "api.acme.com/x",
        "user@api.acme.com",
        "API.acme.com",
        "api.acme.com:0",
        "api.acme.com:",
        "",
        "[::1]",
        "api.acme.com?x",
    ] {
        assert!(
            matches!(AllowedHost::parse(entry), Err(GrantFault::Malformed(_))),
            "{entry}"
        );
    }
    assert_eq!(
        NetGrant::new(Egress::Server, &["a.io", "a.io"]),
        Err(GrantFault::Duplicate("a.io".to_string()))
    );
    assert!(NetGrant::new(Egress::Server, &["a.io", "a.io:8080"]).is_ok());
}

#[test]
fn private_is_a_host_egress_grant_only() {
    assert!(host_grant(true, &["a.io"]).private());
    assert!(!host_grant(false, &["a.io"]).private());
    assert!(!NetGrant::new(Egress::Server, &["a.io"]).unwrap().private());
}
