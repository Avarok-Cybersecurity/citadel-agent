//! What host egress needs from the world, as ports: a resolver, a connector and a clock.
//!
//! The decision logic (net/fetch.rs) reaches the network only through these, so it is tested
//! end to end against fakes that record every attempt and open no socket. Production uses
//! the system resolver and reqwest (net/system.rs).

use async_trait::async_trait;
use std::net::{IpAddr, SocketAddr};

/// Names to addresses. Asked once per hop, and only for a host the policy has already
/// admitted; its answer is decided address by address before anything connects.
#[async_trait]
pub(crate) trait Resolver: Send + Sync {
    /// Every address `host` resolves to; empty or `Err` is refused as "no address".
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<IpAddr>, String>;
}

/// A request the policy allowed, pinned to the addresses it checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PinnedRequest {
    pub method: String,
    /// Rebuilt from the checked target: `scheme://host:port/path?query`.
    pub url: String,
    /// The TLS server name and `Host`.
    pub host: String,
    /// The connector connects to these and nowhere else; it never resolves `host` itself.
    pub connect: Vec<SocketAddr>,
    /// Exactly the app's headers, which the policy allowed. Nothing is added.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// An upstream response whose head has arrived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UpstreamHead {
    pub status: u16,
    /// Lowercase names.
    pub headers: Vec<(String, String)>,
}

impl UpstreamHead {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// The body of an upstream response, read chunk by chunk so a cap stops it early.
#[async_trait]
pub(crate) trait UpstreamBody: Send {
    /// The next chunk; `None` at the end.
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, String>;
}

pub(crate) struct Upstream {
    pub head: UpstreamHead,
    pub body: Box<dyn UpstreamBody>,
}

/// Opens one HTTP exchange to a pinned request's addresses. No redirect is followed here,
/// no proxy is used, and nothing is added to the request: no cookie jar, no credentials.
#[async_trait]
pub(crate) trait Connector: Send + Sync {
    async fn open(&self, request: PinnedRequest) -> Result<Upstream, String>;
}

/// Milliseconds on a monotonic clock, for the rate limit.
pub(crate) trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

/// Everything host egress reaches the world through.
pub(crate) struct NetPorts {
    pub resolver: Box<dyn Resolver>,
    pub connector: Box<dyn Connector>,
    pub clock: Box<dyn Clock>,
}
