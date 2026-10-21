mod decide_tests;
mod grant_tests;
mod hard_deny_tests;
mod range_tests;
mod redirect_tests;
mod url_tests;

use crate::*;
use std::net::IpAddr;

pub(super) const PAGE: &str = "acme.work.avarok.net";

pub(super) fn deployment() -> Deployment {
    Deployment {
        page_origin_host: PAGE.to_string(),
    }
}

pub(super) fn host_grant(private: bool, hosts: &[&str]) -> NetGrant {
    NetGrant::new(Egress::Host { private }, hosts).expect("a valid grant")
}

pub(super) fn get(url: &str) -> EgressRequest<'_> {
    EgressRequest {
        method: "GET",
        url,
        headers: &[],
        body_len: 0,
    }
}

pub(super) fn ip(text: &str) -> IpAddr {
    text.parse().expect(text)
}

/// The verdict for a GET of `url` resolved to `addresses`.
pub(super) fn verdict(grant: &NetGrant, url: &str, addresses: &[&str]) -> Verdict {
    let resolved: Vec<IpAddr> = addresses.iter().map(|a| ip(a)).collect();
    decide(&get(url), &resolved, grant, &deployment())
}

pub(super) fn reason(verdict: Verdict) -> DenyReason {
    match verdict {
        Verdict::Policy(reason) => reason,
        Verdict::Allow(allowed) => panic!("allowed: {allowed:?}"),
    }
}
