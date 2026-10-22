//! What the agent does for Citadel apps (citadel-workspace `docs/plans/
//! citadel-apps-and-coupons-design-r2.md`, R4): host egress (`AppFetch`, net/), `local`
//! storage (`AppKv`, kv/) and the internal-HTTP relay (`IhttpSend`, ihttp/).
//!
//! None of it listens. Host egress is outbound only and refuses the agent's own port as a
//! target; internal HTTP rides the account's existing, authenticated P2P sessions.

pub(crate) mod ihttp;
pub(crate) mod kv;
pub(crate) mod net;

#[cfg(test)]
mod service_tests;

pub use net::AppNetConfig;

use std::sync::Arc;

/// The apps state of one agent.
#[derive(Default)]
pub(crate) struct AppsHost {
    /// `None` until the agent binary configures host egress (`with_app_net`), and for a
    /// configuration with no page origin: `AppFetch` is then refused as `NotConfigured`.
    pub(crate) net: Option<Arc<net::AppNet>>,
    pub(crate) streams: net::stream::Streams,
    pub(crate) kv: kv::AppKv,
    pub(crate) ihttp: ihttp::IhttpCounters,
}

impl AppsHost {
    pub(crate) fn with_net(config: AppNetConfig) -> Self {
        Self {
            net: net::AppNet::new(config, net::system_ports()).map(Arc::new),
            ..Self::default()
        }
    }
}
