//! The fake release server and recorders for tests/updater_engine.rs.

use async_trait::async_trait;
use citadel_internal_service::updater::io::*;
use citadel_internal_service::updater::release::DOWNLOAD_PREFIX;
use citadel_internal_service::updater::verify::Provenance;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

mod staging;
mod world;
pub use world::*;

pub const ETAG: &str = "W/\"fixture\"";

#[derive(Default)]
pub struct FakeSource {
    release: Mutex<serde_json::Value>,
    files: Mutex<HashMap<String, Vec<u8>>>,
    attested: AtomicBool,
    fail_latest: AtomicBool,
    fail_downloads: AtomicBool,
    downloads: AtomicUsize,
    not_modified: AtomicUsize,
    attestation_queries: AtomicUsize,
}

impl FakeSource {
    fn url(tag: &str, name: &str) -> String {
        format!("{DOWNLOAD_PREFIX}{tag}/{name}")
    }

    /// A release `tag` whose every asset name is served with `payload` and its checksum.
    pub fn new(tag: &str, names: &[&str], payload: &[u8]) -> Self {
        let source = Self::default();
        let mut assets = Vec::new();
        for name in names {
            let digest = hex::encode(Sha256::digest(payload));
            let checksum = format!("{digest}  {name}\n").into_bytes();
            for (asset, bytes) in [
                (name.to_string(), payload.to_vec()),
                (format!("{name}.sha256"), checksum),
            ] {
                assets.push(serde_json::json!({
                    "name": asset, "size": bytes.len(), "browser_download_url": Self::url(tag, &asset),
                }));
                source.files.lock().insert(Self::url(tag, &asset), bytes);
            }
        }
        *source.release.lock() = serde_json::json!({
            "tag_name": tag, "draft": false, "prerelease": false, "assets": assets,
            "html_url": format!("https://github.com/x/y/releases/tag/{tag}"),
        });
        source.attested.store(true, Ordering::SeqCst);
        source
    }

    fn tag(&self) -> String {
        self.release.lock()["tag_name"]
            .as_str()
            .unwrap()
            .to_string()
    }

    pub fn wrong_checksum(&self, name: &str) {
        let other = hex::encode(Sha256::digest(b"something else"));
        let url = Self::url(&self.tag(), &format!("{name}.sha256"));
        self.files
            .lock()
            .insert(url, format!("{other}  {name}\n").into_bytes());
    }
    pub fn drop_asset(&self, name: &str) {
        let mut release = self.release.lock();
        let assets = release["assets"].as_array_mut().unwrap();
        assets.retain(|a| a["name"] != name);
    }
    pub fn no_attestations(&self) {
        self.attested.store(false, Ordering::SeqCst);
    }
    pub fn fail_latest(&self, on: bool) {
        self.fail_latest.store(on, Ordering::SeqCst);
    }
    pub fn fail_downloads(&self, on: bool) {
        self.fail_downloads.store(on, Ordering::SeqCst);
    }
    pub fn downloads(&self) -> usize {
        self.downloads.load(Ordering::SeqCst)
    }
    pub fn not_modified(&self) -> usize {
        self.not_modified.load(Ordering::SeqCst)
    }
    pub fn attestation_queries(&self) -> usize {
        self.attestation_queries.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ReleaseSource for FakeSource {
    async fn latest(&self, etag: Option<String>) -> Result<Fetched, String> {
        if self.fail_latest.load(Ordering::SeqCst) {
            return Err("connection refused".to_string());
        }
        if etag.as_deref() == Some(ETAG) {
            self.not_modified.fetch_add(1, Ordering::SeqCst);
            return Ok(Fetched::NotModified);
        }
        let body = serde_json::to_vec(&*self.release.lock()).unwrap();
        Ok(Fetched::Release {
            body,
            etag: Some(ETAG.to_string()),
        })
    }
    async fn text(&self, url: &str, _max: u64) -> Result<String, String> {
        let bytes = self
            .files
            .lock()
            .get(url)
            .cloned()
            .ok_or(format!("404 {url}"))?;
        Ok(String::from_utf8(bytes).unwrap())
    }
    async fn download(&self, url: &str, dest: &Path, size: u64) -> Result<(), String> {
        if self.fail_downloads.load(Ordering::SeqCst) {
            return Err("connection reset".to_string());
        }
        self.downloads.fetch_add(1, Ordering::SeqCst);
        let bytes = self
            .files
            .lock()
            .get(url)
            .cloned()
            .ok_or(format!("404 {url}"))?;
        assert_eq!(bytes.len() as u64, size);
        std::fs::write(dest, bytes).map_err(|e| e.to_string())
    }
    async fn attestations(&self, _sha256: &str) -> Result<Vec<String>, String> {
        self.attestation_queries.fetch_add(1, Ordering::SeqCst);
        Ok(if self.attested.load(Ordering::SeqCst) {
            vec!["bundle".to_string()]
        } else {
            Vec::new()
        })
    }
}

/// Verified when there is a bundle: the bytes it vouches for are the ones the checksum matched.
pub struct FakeVerifier;
impl AttestationVerifier for FakeVerifier {
    fn verify(&self, _sha256: &str, bundles: &[String], _tag: &str) -> Provenance {
        if bundles.is_empty() {
            Provenance::Absent
        } else {
            Provenance::Verified
        }
    }
}
