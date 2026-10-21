//! What no grant reaches, `net.host.private` included, each beside the nearest thing that IS
//! reached, so the refusal is shown to be the deny rule and not something else.

use super::*;

#[test]
fn the_agent_port_is_refused_even_listed_and_private() {
    let grant = host_grant(
        true,
        &["localhost:12345", "localhost:12346", "10.0.0.5:12345"],
    );
    for url in [
        "http://localhost:12345/",
        "https://localhost:12345/",
        "http://10.0.0.5:12345/",
    ] {
        assert_eq!(
            reason(verdict(&grant, url, &["127.0.0.1"])),
            DenyReason::AgentPort,
            "{url}"
        );
    }
    for address in [
        "127.0.0.1",
        "::1",
        "::ffff:127.0.0.1",
        "0.0.0.0",
        "127.4.5.6",
    ] {
        let refused = verdict(&grant, "http://localhost:12345/", &[address]);
        assert_eq!(reason(refused), DenyReason::AgentPort, "{address}");
    }
    // Control: the next port on loopback, listed and under private, is reached.
    assert!(verdict(&grant, "http://localhost:12346/", &["127.0.0.1"]).is_allowed());
}

#[test]
fn the_agent_loopback_name_and_names_below_it_are_refused_even_listed() {
    let grant = host_grant(
        true,
        &["local.avarok.net", "x.local.avarok.net", "avarok.net"],
    );
    for url in [
        "https://local.avarok.net/",
        "https://x.local.avarok.net/",
        "https://LOCAL.avarok.net/",
    ] {
        assert_eq!(
            reason(verdict(&grant, url, &["127.0.0.1"])),
            DenyReason::AgentHost,
            "{url}"
        );
    }
    // Control: the parent name, listed, is reached at a public address.
    assert!(verdict(&grant, "https://avarok.net/", &["104.16.1.1"]).is_allowed());
}

#[test]
fn the_page_origin_is_refused_even_listed() {
    let grant = host_grant(true, &[PAGE, "other.work.avarok.net"]);
    let refused = verdict(&grant, &format!("https://{PAGE}/api"), &["104.16.1.1"]);
    assert_eq!(reason(refused), DenyReason::PageOrigin);
    // Control: another tenant's host, listed, is reached.
    assert!(verdict(&grant, "https://other.work.avarok.net/", &["104.16.1.1"]).is_allowed());
}

#[test]
fn metadata_endpoints_are_refused_even_private_in_every_form() {
    let grant = host_grant(true, &["meta.acme.com", "169.254.169.254"]);
    for address in [
        "169.254.169.254",
        "::ffff:169.254.169.254",
        "64:ff9b::a9fe:a9fe",
        "2002:a9fe:a9fe::1",
        "100.100.100.200",
        "fd00:ec2::254",
    ] {
        let refused = verdict(&grant, "https://meta.acme.com/latest", &[address]);
        assert!(
            matches!(reason(refused), DenyReason::Metadata(_)),
            "{address}"
        );
    }
    let literal = verdict(&grant, "https://169.254.169.254/", &[]);
    assert!(matches!(reason(literal), DenyReason::Metadata(_)));
    // Control: a neighbouring link-local address is reached under private.
    assert!(verdict(&grant, "https://meta.acme.com/", &["169.254.169.253"]).is_allowed());
}

#[test]
fn a_literal_host_is_its_own_address_whatever_the_caller_resolved() {
    let grant = host_grant(false, &["10.0.0.5", "93.184.216.34"]);
    let refused = verdict(&grant, "https://10.0.0.5/", &["93.184.216.34"]);
    assert!(matches!(reason(refused), DenyReason::Address(..)));
    let Verdict::Allow(allowed) = verdict(&grant, "https://93.184.216.34/", &["10.0.0.5"]) else {
        panic!("refused")
    };
    assert_eq!(allowed.connect, vec!["93.184.216.34:443".parse().unwrap()]);
}

#[test]
fn every_refusal_tells_the_app_the_same_word() {
    // The verdict carries no app-facing detail; the one word is the constant.
    assert_eq!(POLICY_REFUSAL, "policy");
    let grant = host_grant(false, &["api.acme.com"]);
    for refused in [
        verdict(&grant, "https://evil.com/", &["8.8.8.8"]),
        verdict(&grant, "https://api.acme.com/", &["10.0.0.1"]),
        verdict(&grant, "https://local.avarok.net/", &["127.0.0.1"]),
        verdict(&grant, "ftp://api.acme.com/", &["8.8.8.8"]),
    ] {
        assert!(matches!(refused, Verdict::Policy(_)));
    }
}
