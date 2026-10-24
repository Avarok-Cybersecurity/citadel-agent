//! Host egress (`net.host`): an app's HTTPS request, made by the member's own agent.
//!
//! Outbound only. Nothing here listens, proxies or tunnels; the agent's own port is refused as
//! a target by the shared policy, on every address, whatever the grant.
//!
//! Every request goes through [`AppNet::open`]: what the workspace server says the install may
//! reach (authorize.rs), narrowed by what the page asks; then the rate limit, then the policy
//! (`citadel_app_net_policy::decide`) before any name is resolved, then the resolver, then the
//! policy again over every address the name resolved to, then one connection to the checked
//! addresses and nowhere else. A redirect is decided again (`decide_redirect`: same host, at
//! most three) and resolved and pinned again. Whatever the policy refuses is answered with the
//! one `Policy` refusal, before anything connects.

pub(crate) mod authority;
mod authorize;
mod body;
mod fetch;
pub(crate) mod grants;
pub(crate) mod meter;
pub(crate) mod ports;
pub(crate) mod rate;
pub(crate) mod stream;
mod system;

#[cfg(test)]
mod tests;

pub(crate) use system::system_ports;

use authority::HostNetAuthority;
use citadel_app_net_policy::Deployment;
use grants::GrantCache;
use meter::Meter;
use ports::NetPorts;
use rate::RateLimiter;
use std::sync::Arc;

/// Where this agent runs, which no app may reach. Required, not defaulted: an agent that does
/// not know the page origins it serves has not been configured for host egress, and refuses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppNetConfig {
    /// The hosts the agent accepts pages from (its origin allowlist, without scheme or port).
    /// Each is refused as a target; at least one.
    pub page_origin_hosts: Vec<String>,
    /// The port the agent's own control socket is bound to: refused on every address, as the
    /// policy refuses `AGENT_PORT`, in case the agent was bound elsewhere.
    pub agent_port: u16,
}

/// Host egress, with the ports it reaches the world through.
pub(crate) struct AppNet {
    deployments: Vec<Deployment>,
    agent_port: u16,
    ports: NetPorts,
    rate: RateLimiter,
    pub(crate) meter: Meter,
    /// What the server last said each install may reach (grants.rs).
    pub(crate) grants: Arc<GrantCache>,
}

impl AppNet {
    /// `None` for a configuration with no page origin host: such an agent refuses host
    /// egress (`NotConfigured`) rather than guessing which origin is its own.
    pub(crate) fn new(
        config: AppNetConfig,
        ports: NetPorts,
        authority: Arc<dyn HostNetAuthority>,
    ) -> Option<Self> {
        if config.page_origin_hosts.is_empty() {
            return None;
        }
        let deployments = config
            .page_origin_hosts
            .into_iter()
            .map(|host| Deployment {
                page_origin_host: host.to_ascii_lowercase(),
            })
            .collect();
        Some(Self {
            deployments,
            agent_port: config.agent_port,
            ports,
            rate: RateLimiter::default(),
            meter: Meter::default(),
            grants: Arc::new(GrantCache::new(authority)),
        })
    }
}

/// An app's request, as the agent was handed it.
pub(crate) struct FetchInput<'a> {
    pub key: &'a rate::InstallKey,
    /// The app the page says it is; refused unless the server says the install is of it.
    pub app_id: &'a str,
    /// What the page asks to reach: a request, never more than the server allows.
    pub grant: &'a citadel_internal_service_types::AppNetGrant,
    pub request: &'a citadel_internal_service_types::AppHttpRequest,
    /// The member's own grants on this agent (kernel/apps/settings.rs).
    pub member: &'a dyn crate::kernel::apps::settings::MemberGrants,
    /// The session's link to its server, which the question about the install goes over.
    pub link: &'a dyn crate::kernel::server_link::SessionLink,
}
