//! The decision: may this request leave, and to which addresses.

use crate::grant::{Deployment, NetGrant};
use crate::headers::headers_allowed;
use crate::ranges::{canonical, classify, AddressClass};
use crate::url::{Scheme, TargetUrl, UrlFault};
use crate::{AGENT_LOOPBACK_HOST, AGENT_PORT, MAX_REDIRECTS, MAX_REQUEST_BODY_BYTES};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// Cloud metadata endpoints: AWS/GCP/Azure, Alibaba, AWS over IPv6.
const METADATA: [IpAddr; 3] = [
    IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)),
    IpAddr::V4(Ipv4Addr::new(100, 100, 100, 200)),
    IpAddr::V6(Ipv6Addr::new(0xfd00, 0xec2, 0, 0, 0, 0, 0, 0x254)),
];

/// The request methods an app may use, for egress and for internal-HTTP routes alike.
pub const METHODS: [&str; 6] = ["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD"];

/// What an app asked for.
#[derive(Debug, Clone, Copy)]
pub struct EgressRequest<'a> {
    /// Uppercase: `GET`, `POST`, `PUT`, `DELETE`, `PATCH` or `HEAD`.
    pub method: &'a str,
    pub url: &'a str,
    /// Lowercase names; see `ALLOWED_HEADERS`.
    pub headers: &'a [(String, String)],
    pub body_len: u64,
}

/// Why the policy refused. For the operator's log and tests only: the app is told
/// `POLICY_REFUSAL`, whatever this says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DenyReason {
    Url(UrlFault),
    /// http without `net.host.private`.
    Scheme,
    Method,
    Body,
    Headers,
    /// The host, or the port on it, is not in the grant.
    NotListed,
    /// The agent's loopback name, or a name below it.
    AgentHost,
    PageOrigin,
    /// The caller resolved the name to nothing.
    NoAddress,
    AgentPort,
    Metadata(IpAddr),
    Address(IpAddr, AddressClass),
    TooManyRedirects,
    RedirectOffHost,
    RedirectDowngrade,
}

/// A request the policy allows. The caller connects to `connect` and nowhere else, sending
/// `target.host` as the TLS server name and `Host`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Allowed {
    pub target: TargetUrl,
    pub connect: Vec<SocketAddr>,
}

impl Allowed {
    /// A `Location` header made absolute against this request: an absolute-path location is
    /// joined to this request's scheme, host and port; anything else is returned as it is, to
    /// be parsed (and refused if it is not a URL) by `decide_redirect`.
    pub fn follow(&self, location: &str) -> String {
        if location.starts_with('/') && !location.starts_with("//") {
            let scheme = match self.target.scheme {
                Scheme::Https => "https",
                Scheme::Http => "http",
            };
            return format!(
                "{scheme}://{}:{}{location}",
                self.target.host, self.target.port
            );
        }
        location.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Allow(Allowed),
    Policy(DenyReason),
}

impl Verdict {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allow(_))
    }
}

fn is_agent_host(host: &str) -> bool {
    host == AGENT_LOOPBACK_HOST || host.ends_with(&format!(".{AGENT_LOOPBACK_HOST}"))
}

/// Decide `request`, whose host the caller resolved to `resolved` (ignored for an IPv4 literal
/// host, which is its own address), under `grant`.
pub fn decide(
    request: &EgressRequest<'_>,
    resolved: &[IpAddr],
    grant: &NetGrant,
    deployment: &Deployment,
) -> Verdict {
    match check(request, resolved, grant, deployment) {
        Ok(allowed) => Verdict::Allow(allowed),
        Err(reason) => Verdict::Policy(reason),
    }
}

/// Decide hop `hop` (1 for the first redirect) from `previous` to `next`: the same host only,
/// never https to http, at most `MAX_REDIRECTS`, and then everything `decide` checks.
pub fn decide_redirect(
    previous: &Allowed,
    hop: u8,
    next: &EgressRequest<'_>,
    resolved: &[IpAddr],
    grant: &NetGrant,
    deployment: &Deployment,
) -> Verdict {
    if hop == 0 || hop > MAX_REDIRECTS {
        return Verdict::Policy(DenyReason::TooManyRedirects);
    }
    let target = match TargetUrl::parse(next.url) {
        Ok(target) => target,
        Err(fault) => return Verdict::Policy(DenyReason::Url(fault)),
    };
    if target.host != previous.target.host {
        return Verdict::Policy(DenyReason::RedirectOffHost);
    }
    if previous.target.scheme == Scheme::Https && target.scheme == Scheme::Http {
        return Verdict::Policy(DenyReason::RedirectDowngrade);
    }
    decide(next, resolved, grant, deployment)
}

fn check(
    request: &EgressRequest<'_>,
    resolved: &[IpAddr],
    grant: &NetGrant,
    deployment: &Deployment,
) -> Result<Allowed, DenyReason> {
    let target = TargetUrl::parse(request.url).map_err(DenyReason::Url)?;
    // The deployment's own names first, before the grant is even read: no grant reaches them.
    if is_agent_host(&target.host) {
        return Err(DenyReason::AgentHost);
    }
    if target.host == deployment.page_origin_host {
        return Err(DenyReason::PageOrigin);
    }
    if target.port == AGENT_PORT {
        return Err(DenyReason::AgentPort);
    }
    if target.scheme == Scheme::Http && !grant.private() {
        return Err(DenyReason::Scheme);
    }
    if !METHODS.contains(&request.method) {
        return Err(DenyReason::Method);
    }
    let bodiless = matches!(request.method, "GET" | "HEAD");
    if request.body_len > MAX_REQUEST_BODY_BYTES || (bodiless && request.body_len > 0) {
        return Err(DenyReason::Body);
    }
    if !headers_allowed(request.headers) {
        return Err(DenyReason::Headers);
    }
    if !grant.admits(&target.host, target.port) {
        return Err(DenyReason::NotListed);
    }
    let addresses: Vec<IpAddr> = match target.ip_literal() {
        Some(literal) => vec![IpAddr::V4(literal)],
        None => resolved.to_vec(),
    };
    if addresses.is_empty() {
        return Err(DenyReason::NoAddress);
    }
    for address in &addresses {
        check_address(*address, grant)?;
    }
    let connect = addresses
        .into_iter()
        .map(|ip| SocketAddr::new(ip, target.port))
        .collect();
    Ok(Allowed { target, connect })
}

fn check_address(address: IpAddr, grant: &NetGrant) -> Result<(), DenyReason> {
    let plain = canonical(address);
    if METADATA.contains(&plain) || METADATA.contains(&address) {
        return Err(DenyReason::Metadata(address));
    }
    match classify(address) {
        AddressClass::Public => Ok(()),
        AddressClass::Private(_) if grant.private() => Ok(()),
        class => Err(DenyReason::Address(address, class)),
    }
}
