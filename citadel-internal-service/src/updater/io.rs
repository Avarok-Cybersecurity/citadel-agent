//! The updater's seams (SBIO): everything that touches the network, the disk, other processes
//! or the agent's windows. The engine decides; these do.

use super::platform::Method;
use super::verify::Provenance;
use async_trait::async_trait;
use citadel_internal_service_types::UpdateAvailable;
use semver::Version;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub enum Fetched {
    /// The ETag still matches: the release last read is still the latest.
    NotModified,
    Release {
        body: Vec<u8>,
        etag: Option<String>,
    },
}

/// GitHub, or a test's fixture server.
#[async_trait]
pub trait ReleaseSource: Send + Sync {
    /// `releases/latest`, conditional on `etag`.
    async fn latest(&self, etag: Option<String>) -> Result<Fetched, String>;
    /// A small text asset (a `.sha256`), at most `max_bytes`.
    async fn text(&self, url: &str, max_bytes: u64) -> Result<String, String>;
    /// `url` to `dest`, which must come to exactly `size` bytes.
    async fn download(&self, url: &str, dest: &Path, size: u64) -> Result<(), String>;
    /// The Sigstore bundles GitHub holds for the file with this sha256; empty when it has none.
    async fn attestations(&self, sha256: &str) -> Result<Vec<String>, String>;
}

/// Verifies attestation bundles for the file with digest `sha256`, released as `tag`.
pub trait AttestationVerifier: Send + Sync {
    fn verify(&self, sha256: &str, bundles: &[String], tag: &str) -> Provenance;
}

/// The updater's own directory: downloads, staged copies, and versions that failed to install.
#[async_trait]
pub trait Staging: Send + Sync {
    /// Where `asset` of `version` is downloaded to. Anything staged for another version goes.
    fn download_path(&self, version: &Version, asset: &str) -> Result<PathBuf, String>;
    async fn sha256(&self, path: &Path) -> Result<String, String>;
    /// The runnable form of a verified download: the tarball's binary, the AppImage made
    /// executable, the disk image as it is.
    async fn stage(&self, method: Method, download: &Path) -> Result<PathBuf, String>;
    /// What `runnable --version` printed.
    async fn version_of(&self, runnable: &Path) -> Result<String, String>;
    /// Remove what was downloaded for `version`.
    fn discard(&self, version: &Version);
    fn failed(&self, version: &Version) -> bool;
    fn mark_failed(&self, version: &Version);
}

/// Puts a staged, verified release in place of this agent and restarts it.
pub trait Installer: Send + Sync {
    /// Why it could not install right now, if it could not.
    fn can_install(&self) -> Result<(), String>;
    /// On success the agent is about to be replaced: a self-replacing installer exits the
    /// process, the menu-bar app's stops it.
    fn install(&self, staged: &Path, version: &Version) -> Result<(), String>;
}

/// Tells the windows and the menu-bar app.
pub trait Announcer: Send + Sync {
    fn announce(&self, update: &UpdateAvailable);
}

/// "Automatically install updates when no account is signed in", in the agent's preferences.
#[async_trait]
pub trait SettingsStore: Send + Sync {
    /// `None` when it was never set.
    async fn auto_install(&self) -> Result<Option<bool>, String>;
    async fn set_auto_install(&self, on: bool) -> Result<(), String>;
}

pub trait Sessions: Send + Sync {
    fn signed_in(&self) -> usize;
}

#[derive(Clone)]
pub struct Io {
    pub source: Arc<dyn ReleaseSource>,
    pub verifier: Arc<dyn AttestationVerifier>,
    pub staging: Arc<dyn Staging>,
    pub installer: Arc<dyn Installer>,
    pub announcer: Arc<dyn Announcer>,
    pub settings: Arc<dyn SettingsStore>,
    pub sessions: Arc<dyn Sessions>,
    /// The ML-DSA-65 public key (hex) every release asset's `.mldsa.sig` must verify against:
    /// `release_key::RELEASE_PUBLIC_KEY` in the shipped agent.
    pub release_key: Arc<str>,
    /// Seconds since the Unix epoch.
    pub now: fn() -> u64,
}
