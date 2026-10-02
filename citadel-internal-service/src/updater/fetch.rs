//! Download, verify and stage one release asset. Nothing from it runs before its checksum and
//! its attestation have passed.

use super::engine::{Engine, LOG_TARGET};
use super::platform::Method;
use super::release::{self, Release};
use super::verify::{expected_sha256, judge_download, judge_staged, Provenance, Verdict};
use citadel_sdk::logging::info;
use semver::Version;
use std::path::PathBuf;

/// The largest asset the updater downloads. The disk image is ~25 MB today.
pub const MAX_ASSET_BYTES: u64 = 512 * 1024 * 1024;
const MAX_CHECKSUM_BYTES: u64 = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The network or the disk failed: try again at the next check.
    Transient(String),
    /// Not unsafe, but not to be installed automatically: offer the download.
    LinkOnly(String),
    /// Failed verification: never this file.
    Refused(String),
}

impl Engine {
    pub(super) async fn fetch_and_verify(
        &self,
        release: &Release,
        version: &Version,
        name: &str,
        method: Method,
    ) -> Result<PathBuf, Refusal> {
        let refused = Refusal::Refused;
        let asset = release::asset(release, name).map_err(refused)?;
        let checksum = release::asset(release, &format!("{name}.sha256")).map_err(refused)?;
        if asset.size > MAX_ASSET_BYTES {
            return Err(refused(format!(
                "{name} is {} bytes, over the {MAX_ASSET_BYTES} the updater accepts",
                asset.size
            )));
        }
        let transient = Refusal::Transient;
        let text = self
            .io
            .source
            .text(&checksum.browser_download_url, MAX_CHECKSUM_BYTES)
            .await
            .map_err(transient)?;
        let expected = expected_sha256(&text, name).map_err(refused)?;
        let path = self
            .io
            .staging
            .download_path(version, name)
            .map_err(transient)?;
        info!(target: LOG_TARGET, "downloading {name} for {version}");
        self.io
            .source
            .download(&asset.browser_download_url, &path, asset.size)
            .await
            .map_err(transient)?;
        let actual = self.io.staging.sha256(&path).await.map_err(transient)?;
        let provenance = if expected.eq_ignore_ascii_case(&actual) {
            let bundles = self
                .io
                .source
                .attestations(&actual)
                .await
                .map_err(transient)?;
            self.io
                .verifier
                .verify(&actual, &bundles, &release.tag_name)
        } else {
            // Not consulted: the checksum already refuses it.
            Provenance::Absent
        };
        let verdict = judge_download(
            &self.current,
            version,
            &expected,
            &actual,
            &provenance,
            method,
        );
        settle(verdict)?;
        let runnable = self
            .io
            .staging
            .stage(method, &path)
            .await
            .map_err(refused)?;
        let printed = match method {
            Method::MacApp => None,
            Method::Tarball | Method::AppImage => Some(
                self.io
                    .staging
                    .version_of(&runnable)
                    .await
                    .map_err(refused)?,
            ),
        };
        settle(judge_staged(version, printed.as_deref(), method))?;
        Ok(runnable)
    }
}

fn settle(verdict: Verdict) -> Result<(), Refusal> {
    match verdict {
        Verdict::Accept => Ok(()),
        Verdict::LinkOnly(why) => Err(Refusal::LinkOnly(why)),
        Verdict::Refuse(why) => Err(Refusal::Refused(why)),
    }
}
