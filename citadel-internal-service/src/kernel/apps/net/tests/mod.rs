//! Host egress end to end over in-process fakes: a resolver that answers what the test says,
//! a connector that records every attempt and opens nothing, and a clock the test sets. No
//! test here touches the network.

mod allowed;
mod denied;
mod limits;
mod private_network;
mod redirects;
mod streaming;

use super::ports::{
    Clock, Connector, NetPorts, PinnedRequest, Resolver, Upstream, UpstreamBody, UpstreamHead,
};
use super::rate::InstallKey;
use super::{AppNet, AppNetConfig, FetchInput};
use async_trait::async_trait;
use citadel_internal_service_types::{AppHttpHeader, AppHttpRequest, AppNetGrant};
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

pub(super) const PAGE: &str = "acme.work.avarok.net";
pub(super) const APP: &str = "com.acme.kanban";

/// Answers per host, in order; the last answer repeats. Counts every lookup.
#[derive(Default)]
pub(super) struct FakeResolver {
    answers: Mutex<HashMap<String, VecDeque<Vec<IpAddr>>>>,
    pub lookups: Mutex<Vec<String>>,
}

impl FakeResolver {
    pub(super) fn answer(&self, host: &str, addresses: &[&str]) {
        let parsed = addresses.iter().map(|a| a.parse().expect(a)).collect();
        self.answers
            .lock()
            .entry(host.to_string())
            .or_default()
            .push_back(parsed);
    }
}

#[async_trait]
impl Resolver for Arc<FakeResolver> {
    async fn resolve(&self, host: &str, _port: u16) -> Result<Vec<IpAddr>, String> {
        self.lookups.lock().push(host.to_string());
        let mut answers = self.answers.lock();
        let queue = answers.get_mut(host).ok_or("NXDOMAIN")?;
        match queue.len() {
            0 => Err("NXDOMAIN".to_string()),
            1 => Ok(queue[0].clone()),
            _ => Ok(queue.pop_front().unwrap_or_default()),
        }
    }
}

/// A scripted response: status, headers, and the body's chunks.
#[derive(Clone)]
pub(super) struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub chunks: Vec<Vec<u8>>,
}

pub(super) fn ok(body: &[u8]) -> Reply {
    Reply {
        status: 200,
        headers: vec![("content-type".into(), "text/plain".into())],
        chunks: vec![body.to_vec()],
    }
}

pub(super) fn redirect(status: u16, location: &str) -> Reply {
    Reply {
        status,
        headers: vec![("location".into(), location.into())],
        chunks: vec![],
    }
}

/// Records every request it is asked to open; replies in script order, then `ok`. A
/// connector that `hangs` never answers.
#[derive(Default)]
pub(super) struct FakeConnector {
    pub attempts: Mutex<Vec<PinnedRequest>>,
    script: Mutex<VecDeque<Reply>>,
    pub hangs: std::sync::atomic::AtomicBool,
}

impl FakeConnector {
    pub(super) fn reply(&self, reply: Reply) {
        self.script.lock().push_back(reply);
    }
    pub(super) fn attempts(&self) -> Vec<PinnedRequest> {
        self.attempts.lock().clone()
    }
}

struct ScriptedBody(VecDeque<Vec<u8>>);

#[async_trait]
impl UpstreamBody for ScriptedBody {
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, String> {
        Ok(self.0.pop_front())
    }
}

#[async_trait]
impl Connector for Arc<FakeConnector> {
    async fn open(&self, request: PinnedRequest) -> Result<Upstream, String> {
        self.attempts.lock().push(request);
        if self.hangs.load(Ordering::SeqCst) {
            futures::future::pending::<()>().await;
        }
        let reply = self.script.lock().pop_front().unwrap_or_else(|| ok(b"ok"));
        Ok(Upstream {
            head: UpstreamHead {
                status: reply.status,
                headers: reply.headers,
            },
            body: Box::new(ScriptedBody(reply.chunks.into())),
        })
    }
}

#[derive(Default)]
pub(super) struct FakeClock(pub AtomicU64);

impl Clock for Arc<FakeClock> {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// An `AppNet` over fakes, and the fakes.
pub(super) struct World {
    pub net: Arc<AppNet>,
    pub resolver: Arc<FakeResolver>,
    pub connector: Arc<FakeConnector>,
    pub clock: Arc<FakeClock>,
}

pub(super) fn world_with(agent_port: u16, private_network: bool) -> World {
    let resolver = Arc::new(FakeResolver::default());
    let connector = Arc::new(FakeConnector::default());
    let clock = Arc::new(FakeClock::default());
    let ports = NetPorts {
        resolver: Box::new(resolver.clone()),
        connector: Box::new(connector.clone()),
        clock: Box::new(clock.clone()),
    };
    let config = AppNetConfig {
        page_origin_hosts: vec![PAGE.to_string()],
        agent_port,
        private_network,
    };
    let net = Arc::new(AppNet::new(config, ports).expect("configured"));
    World {
        net,
        resolver,
        connector,
        clock,
    }
}

/// Private network on: most tests here exercise what the policy itself refuses, which it
/// refuses with the ceiling raised; an agent that keeps it off is private_network.rs's.
pub(super) fn world_on_port(agent_port: u16) -> World {
    world_with(agent_port, true)
}

pub(super) fn world() -> World {
    world_on_port(citadel_app_net_policy::AGENT_PORT)
}

pub(super) fn key() -> InstallKey {
    InstallKey {
        cid: 42,
        install_id: "inst-1".to_string(),
    }
}

pub(super) fn grant(private: bool, hosts: &[&str]) -> AppNetGrant {
    AppNetGrant {
        hosts: hosts.iter().map(|h| h.to_string()).collect(),
        private,
    }
}

pub(super) fn get(url: &str) -> AppHttpRequest {
    AppHttpRequest {
        method: "GET".into(),
        url: url.into(),
        headers: vec![AppHttpHeader {
            name: "accept".into(),
            value: "application/json".into(),
        }],
        body: vec![],
    }
}

pub(super) fn deadline() -> Instant {
    Instant::now() + Duration::from_millis(citadel_app_net_policy::REQUEST_TIMEOUT_MS)
}

impl World {
    pub(super) async fn fetch(
        &self,
        grant: &AppNetGrant,
        request: &AppHttpRequest,
    ) -> Result<super::body::Fetched, citadel_internal_service_types::AppFetchRefusal> {
        self.fetch_as(APP, grant, request).await
    }

    pub(super) async fn fetch_as(
        &self,
        app_id: &str,
        grant: &AppNetGrant,
        request: &AppHttpRequest,
    ) -> Result<super::body::Fetched, citadel_internal_service_types::AppFetchRefusal> {
        let key = key();
        let input = FetchInput {
            key: &key,
            app_id,
            grant,
            request,
        };
        self.net.fetch(input, deadline()).await
    }
}
