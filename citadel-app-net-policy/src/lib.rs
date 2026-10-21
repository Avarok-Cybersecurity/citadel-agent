//! The SSRF policy for a Citadel app's network egress, defined once.
//!
//! An app never reaches the network from the page. It asks its host: the member's local agent
//! (`net.host`, and `net.host.private` for LAN APIs) or the workspace server (`net.server`).
//! Whichever runs the request first calls [`decide`] with the request, every address the
//! target's name resolved to, the install's grant and the deployment's own addresses, and then
//! connects ONLY to the addresses the verdict returns (resolve-then-pin, so a second DNS answer
//! cannot rebind the connection to somewhere the policy never saw). Each redirect hop is
//! decided again with [`decide_redirect`].
//!
//! Every refusal is the same [`Verdict::Policy`] to the app, decided before any connection is
//! attempted, so an app cannot map a network by which refusal or how long it took. The
//! [`DenyReason`] inside is for the operator's log and for tests; it never reaches the app.
//!
//! What is refused whatever the grant, `net.host.private` included:
//! - the agent's own port ([`AGENT_PORT`]) on any loopback or unspecified address, and the
//!   name the page uses to reach it ([`AGENT_LOOPBACK_HOST`] and every name below it);
//! - the page's own origin host;
//! - cloud metadata endpoints (`169.254.169.254`, `fd00:ec2::254`);
//! - addresses that are not unicast (this-network, multicast, broadcast, reserved,
//!   documentation, benchmarking, IETF protocol assignments, Teredo, discard).
//!
//! Private, loopback, link-local, shared (CGNAT) and unique-local addresses are refused unless
//! the grant is host egress with `private`; server egress never reaches them. IPv4 written
//! inside IPv6 (mapped `::ffff:a.b.c.d`, NAT64 `64:ff9b::/96`, 6to4 `2002::/16`) is judged as
//! the IPv4 address it carries.

mod decide;
mod grant;
mod headers;
mod ranges;
mod url;

#[cfg(test)]
mod tests;

pub use decide::{decide, decide_redirect, Allowed, DenyReason, EgressRequest, Verdict};
pub use grant::{AllowedHost, Deployment, Egress, GrantFault, NetGrant};
pub use headers::ALLOWED_HEADERS;
pub use ranges::{canonical, classify, AddressClass, ForbiddenRange, PrivateRange};
pub use url::{Scheme, TargetUrl, UrlFault};

/// The local agent's port. Never reachable by an app, on any loopback address.
pub const AGENT_PORT: u16 = 12345;
/// The name the hosted page reaches the local agent by (it resolves to loopback). Refused, with
/// every name below it, whatever the grant.
pub const AGENT_LOOPBACK_HOST: &str = "local.avarok.net";
/// Ports an allow-listed host may be reached on when its entry names no port.
pub const DEFAULT_PORTS: [u16; 2] = [443, 8443];
/// The most hosts one grant may list. Hosts are explicit names; no wildcards.
pub const MAX_HOSTS: usize = 8;
/// The most redirects one request may follow, each decided again.
pub const MAX_REDIRECTS: u8 = 3;
pub const MAX_REQUEST_BODY_BYTES: u64 = 4 * 1024 * 1024;
/// What the caller must stop reading a response at.
pub const MAX_RESPONSE_BODY_BYTES: u64 = 16 * 1024 * 1024;
/// What the caller must abandon a request at, redirects included.
pub const REQUEST_TIMEOUT_MS: u64 = 30_000;
/// The caller's rate limit, per install per user.
pub const MAX_REQUESTS_PER_MINUTE: u32 = 60;
pub const MAX_URL_BYTES: usize = 8 * 1024;
pub const MAX_HEADERS: usize = 32;
/// A header's name and value together.
pub const MAX_HEADER_BYTES: usize = 1024;
/// The one word an app is told when the policy refuses, whatever the reason.
pub const POLICY_REFUSAL: &str = "policy";
