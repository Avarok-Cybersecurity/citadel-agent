//! The production ports: the system resolver, reqwest pinned to the checked addresses, and
//! the monotonic clock.

use super::ports::{
    Clock, Connector, NetPorts, PinnedRequest, Resolver, Upstream, UpstreamBody, UpstreamHead,
};
use async_trait::async_trait;
use std::net::IpAddr;
use std::time::Instant;

struct SystemResolver;

#[async_trait]
impl Resolver for SystemResolver {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<IpAddr>, String> {
        let addresses = tokio::net::lookup_host((host, port))
            .await
            .map_err(|why| why.to_string())?;
        Ok(addresses.map(|address| address.ip()).collect())
    }
}

/// One client per hop, built for that hop's checked addresses: `resolve_to_addrs` makes the
/// name resolve to exactly those, so reqwest never asks DNS again and a second answer cannot
/// rebind the connection. No redirects (each is decided by the policy), no proxy (an
/// environment `HTTPS_PROXY` would send the request somewhere the policy never saw), no
/// cookie store.
struct ReqwestConnector;

#[async_trait]
impl Connector for ReqwestConnector {
    async fn open(&self, request: PinnedRequest) -> Result<Upstream, String> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .resolve_to_addrs(&request.host, &request.connect)
            .build()
            .map_err(|why| why.to_string())?;
        let method = reqwest::Method::from_bytes(request.method.as_bytes())
            .map_err(|why| why.to_string())?;
        let mut builder = client.request(method, &request.url);
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        if !request.body.is_empty() {
            builder = builder.body(request.body);
        }
        let response = builder.send().await.map_err(|why| why.to_string())?;
        let head = UpstreamHead {
            status: response.status().as_u16(),
            headers: response
                .headers()
                .iter()
                .filter_map(|(name, value)| {
                    let value = value.to_str().ok()?;
                    Some((name.as_str().to_string(), value.to_string()))
                })
                .collect(),
        };
        Ok(Upstream {
            head,
            body: Box::new(ReqwestBody(response)),
        })
    }
}

struct ReqwestBody(reqwest::Response);

#[async_trait]
impl UpstreamBody for ReqwestBody {
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, String> {
        self.0
            .chunk()
            .await
            .map(|chunk| chunk.map(|bytes| bytes.to_vec()))
            .map_err(|why| why.to_string())
    }
}

struct MonotonicClock(Instant);

impl Clock for MonotonicClock {
    fn now_ms(&self) -> u64 {
        u64::try_from(self.0.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

/// The ports the agent binary runs host egress through.
pub(crate) fn system_ports() -> NetPorts {
    NetPorts {
        resolver: Box::new(SystemResolver),
        connector: Box::new(ReqwestConnector),
        clock: Box::new(MonotonicClock(Instant::now())),
    }
}
