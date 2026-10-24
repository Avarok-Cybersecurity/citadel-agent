//! The workspace server as host egress meets it: a codec of the test's own (`FakeAuthority`)
//! and a session link (`FakeServer`) that records every frame the agent sends and answers a
//! grant question as the test scripted, by feeding the answer to the agent's cache exactly as
//! the session's reader would (kernel/c2s_reader.rs).

use super::super::authority::{AuthoritySignal, HostNetAuthority, KernelGrant};
use super::super::grants::GrantCache;
use crate::kernel::server_link::SessionLink;
use futures::future::BoxFuture;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

type Wire = (String, Option<(String, Vec<String>, bool)>, String, u64);

/// Frames as JSON tuples: `("q", None, install, tag)` asks; `("g", grant, install, tag)`
/// answers; `("changed", None, "", 0)` says the member's installs changed.
pub(crate) struct FakeAuthority;

impl FakeAuthority {
    pub(crate) fn answer(install_id: &str, tag: u64, grant: &Option<KernelGrant>) -> Vec<u8> {
        let grant = grant
            .as_ref()
            .map(|g| (g.app_id.clone(), g.hosts.clone(), g.private));
        let wire: Wire = ("g".into(), grant, install_id.into(), tag);
        serde_json::to_vec(&wire).expect("encodes")
    }

    pub(crate) fn installs_changed() -> Vec<u8> {
        let wire: Wire = ("changed".into(), None, String::new(), 0);
        serde_json::to_vec(&wire).expect("encodes")
    }

    fn question(frame: &[u8]) -> Option<(String, u64)> {
        let (kind, _, install, tag): Wire = serde_json::from_slice(frame).ok()?;
        (kind == "q").then_some((install, tag))
    }
}

impl HostNetAuthority for FakeAuthority {
    fn grant_query(&self, install_id: &str, tag: u64) -> Result<Vec<u8>, String> {
        let wire: Wire = ("q".into(), None, install_id.into(), tag);
        serde_json::to_vec(&wire).map_err(|e| e.to_string())
    }

    fn classify(&self, frame: &[u8]) -> AuthoritySignal {
        match serde_json::from_slice::<Wire>(frame) {
            Ok((kind, grant, install_id, tag)) if kind == "g" => AuthoritySignal::Grant {
                install_id,
                tag,
                grant: grant.map(|(app_id, hosts, private)| KernelGrant {
                    app_id,
                    hosts,
                    private,
                }),
            },
            Ok((kind, ..)) if kind == "changed" => AuthoritySignal::InstallsChanged,
            _ => AuthoritySignal::Unrelated,
        }
    }
}

/// Answers each install as scripted (`approve`), and any other install with `fallback`.
pub(crate) struct FakeServer {
    cache: Arc<GrantCache>,
    pub sent: Mutex<Vec<(u64, Vec<u8>)>>,
    grants: Mutex<HashMap<String, Option<KernelGrant>>>,
    fallback: KernelGrant,
    /// Takes questions and never answers them.
    pub silent: AtomicBool,
}

impl FakeServer {
    pub(crate) fn new(cache: Arc<GrantCache>, fallback: KernelGrant) -> Self {
        Self {
            cache,
            sent: Mutex::default(),
            grants: Mutex::default(),
            fallback,
            silent: AtomicBool::new(false),
        }
    }

    pub(crate) fn approve(&self, install_id: &str, grant: Option<KernelGrant>) {
        self.grants.lock().insert(install_id.to_string(), grant);
    }

    pub(crate) fn questions(&self) -> usize {
        self.sent.lock().len()
    }
}

impl SessionLink for FakeServer {
    fn send_to_server(&self, cid: u64, frame: Vec<u8>) -> BoxFuture<'static, Result<(), String>> {
        self.sent.lock().push((cid, frame.clone()));
        if !self.silent.load(Ordering::SeqCst) {
            if let Some((install, tag)) = FakeAuthority::question(&frame) {
                let grant = self
                    .grants
                    .lock()
                    .get(&install)
                    .cloned()
                    .unwrap_or_else(|| Some(self.fallback.clone()));
                let consumed = !self
                    .cache
                    .on_server_frame(cid, &FakeAuthority::answer(&install, tag, &grant));
                assert!(
                    consumed,
                    "the agent did not take the answer to its question"
                );
            }
        }
        Box::pin(async { Ok(()) })
    }
}
