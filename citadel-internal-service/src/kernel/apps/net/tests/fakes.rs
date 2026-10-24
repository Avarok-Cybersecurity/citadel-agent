//! The fakes host egress is tested over: a resolver that answers what the test says, a
//! connector that records every attempt and opens nothing, a clock the test sets, and the
//! members' grants as the test sets them.

use super::super::ports::{
    Clock, Connector, PinnedRequest, Resolver, Upstream, UpstreamBody, UpstreamHead,
};
use crate::kernel::apps::settings::MemberGrants;
use async_trait::async_trait;
use futures::future::BoxFuture;
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Answers per host, in order; the last answer repeats. Counts every lookup.
#[derive(Default)]
pub(crate) struct FakeResolver {
    answers: Mutex<HashMap<String, VecDeque<Vec<IpAddr>>>>,
    pub lookups: Mutex<Vec<String>>,
}

impl FakeResolver {
    pub(crate) fn answer(&self, host: &str, addresses: &[&str]) {
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
pub(crate) struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub chunks: Vec<Vec<u8>>,
}

pub(crate) fn ok(body: &[u8]) -> Reply {
    Reply {
        status: 200,
        headers: vec![("content-type".into(), "text/plain".into())],
        chunks: vec![body.to_vec()],
    }
}

pub(crate) fn redirect(status: u16, location: &str) -> Reply {
    Reply {
        status,
        headers: vec![("location".into(), location.into())],
        chunks: vec![],
    }
}

/// Records every request it is asked to open; replies in script order, then `ok`. A
/// connector that `hangs` never answers.
#[derive(Default)]
pub(crate) struct FakeConnector {
    pub attempts: Mutex<Vec<PinnedRequest>>,
    script: Mutex<VecDeque<Reply>>,
    pub hangs: std::sync::atomic::AtomicBool,
}

impl FakeConnector {
    pub(crate) fn reply(&self, reply: Reply) {
        self.script.lock().push_back(reply);
    }
    pub(crate) fn attempts(&self) -> Vec<PinnedRequest> {
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
pub(crate) struct FakeClock(pub AtomicU64);

impl Clock for Arc<FakeClock> {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// Each member's private-network grant, as the test sets it; a member it was not set for has
/// none. The store behind the real grants is requests/apps_settings_tests.rs's.
#[derive(Default)]
pub(crate) struct FakeSettings(Mutex<HashMap<u64, bool>>);

impl FakeSettings {
    pub(crate) fn granting(cid: u64, on: bool) -> Self {
        let settings = Self::default();
        settings.0.lock().insert(cid, on);
        settings
    }
}

impl MemberGrants for FakeSettings {
    fn private_network(&self, cid: u64) -> BoxFuture<'_, Result<Option<bool>, String>> {
        let granted = self.0.lock().get(&cid).copied();
        Box::pin(async move { Ok(granted) })
    }
}
