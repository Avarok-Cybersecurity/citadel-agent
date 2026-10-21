//! Redirects: each hop decided again, the same host only, never down to http, at most three.

use super::*;

fn first(grant: &NetGrant) -> Allowed {
    match verdict(grant, "https://api.acme.com/start", &["93.184.216.34"]) {
        Verdict::Allow(allowed) => allowed,
        Verdict::Policy(reason) => panic!("{reason:?}"),
    }
}

fn hop(grant: &NetGrant, previous: &Allowed, n: u8, url: &str, resolved: &[&str]) -> Verdict {
    let resolved: Vec<IpAddr> = resolved.iter().map(|a| ip(a)).collect();
    decide_redirect(previous, n, &get(url), &resolved, grant, &deployment())
}

#[test]
fn up_to_three_same_host_hops_are_followed() {
    let grant = host_grant(false, &["api.acme.com"]);
    let start = first(&grant);
    for n in 1..=MAX_REDIRECTS {
        assert!(hop(
            &grant,
            &start,
            n,
            "https://api.acme.com/next",
            &["93.184.216.34"]
        )
        .is_allowed());
    }
    let fourth = hop(
        &grant,
        &start,
        MAX_REDIRECTS + 1,
        "https://api.acme.com/next",
        &["93.184.216.34"],
    );
    assert_eq!(reason(fourth), DenyReason::TooManyRedirects);
    let zeroth = hop(
        &grant,
        &start,
        0,
        "https://api.acme.com/next",
        &["93.184.216.34"],
    );
    assert_eq!(reason(zeroth), DenyReason::TooManyRedirects);
}

#[test]
fn a_hop_to_another_host_is_refused_even_a_listed_one() {
    let grant = host_grant(false, &["api.acme.com", "cdn.acme.com"]);
    let start = first(&grant);
    let off = hop(
        &grant,
        &start,
        1,
        "https://cdn.acme.com/x",
        &["93.184.216.35"],
    );
    assert_eq!(reason(off), DenyReason::RedirectOffHost);
    // Control: the same host is followed.
    assert!(hop(
        &grant,
        &start,
        1,
        "https://api.acme.com/x",
        &["93.184.216.34"]
    )
    .is_allowed());
}

#[test]
fn a_hop_down_to_http_is_refused_even_with_private() {
    let grant = host_grant(true, &["api.acme.com", "api.acme.com:80"]);
    let start = first(&grant);
    let down = hop(
        &grant,
        &start,
        1,
        "http://api.acme.com/x",
        &["93.184.216.34"],
    );
    assert_eq!(reason(down), DenyReason::RedirectDowngrade);
}

#[test]
fn each_hop_is_decided_again_so_a_rebound_name_is_refused() {
    let grant = host_grant(false, &["api.acme.com"]);
    let start = first(&grant);
    let rebound = hop(&grant, &start, 1, "https://api.acme.com/x", &["127.0.0.1"]);
    assert!(matches!(reason(rebound), DenyReason::Address(..)));
    let port = hop(
        &grant,
        &start,
        1,
        "https://api.acme.com:12345/x",
        &["93.184.216.34"],
    );
    assert_eq!(reason(port), DenyReason::AgentPort);
    let unlisted_port = hop(
        &grant,
        &start,
        1,
        "https://api.acme.com:9000/x",
        &["93.184.216.34"],
    );
    assert_eq!(reason(unlisted_port), DenyReason::NotListed);
}

#[test]
fn a_relative_location_is_joined_to_the_request_it_answers() {
    let grant = host_grant(false, &["api.acme.com"]);
    let start = first(&grant);
    assert_eq!(
        start.follow("/v2/items"),
        "https://api.acme.com:443/v2/items"
    );
    assert_eq!(start.follow("//evil.com/x"), "//evil.com/x");
    let joined = start.follow("/v2/items");
    assert!(hop(&grant, &start, 1, &joined, &["93.184.216.34"]).is_allowed());
    let scheme_relative = start.follow("//evil.com/x");
    let refused = hop(&grant, &start, 1, &scheme_relative, &["93.184.216.34"]);
    assert!(matches!(reason(refused), DenyReason::Url(_)));
}
