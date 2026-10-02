//! The GitHub release the updater reads, and what in it may be trusted to point where (pure).

use super::version::{is_upgrade, version_of_tag};
use semver::Version;
use serde::Deserialize;

/// The repository agent releases are published from. A constant: nothing on the network or in
/// a request can point the updater anywhere else.
pub const REPO: &str = "Avarok-Cybersecurity/citadel-workspace";
pub const LATEST_URL: &str =
    "https://api.github.com/repos/Avarok-Cybersecurity/citadel-workspace/releases/latest";
/// Where a release asset is downloaded from: `<this><tag>/<name>`.
pub const DOWNLOAD_PREFIX: &str =
    "https://github.com/Avarok-Cybersecurity/citadel-workspace/releases/download/";
pub const RELEASE_PAGE_PREFIX: &str =
    "https://github.com/Avarok-Cybersecurity/citadel-workspace/releases/tag/";

/// The hosts any request, or any redirect it follows, may reach. `release-assets` is where
/// GitHub redirects release downloads today; `objects` is where it did before.
pub const ALLOWED_HOSTS: [&str; 4] = [
    "api.github.com",
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Release {
    pub tag_name: String,
    pub draft: bool,
    pub prerelease: bool,
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
}

/// What the latest release means for an agent at `current`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Candidate {
    /// Nothing to offer, and why.
    None(String),
    Upgrade(Version),
}

pub fn parse(body: &[u8]) -> Result<Release, String> {
    serde_json::from_slice(body).map_err(|e| format!("the release JSON did not parse: {e}"))
}

pub fn candidate(release: &Release, current: &Version) -> Candidate {
    if release.draft || release.prerelease {
        return Candidate::None(format!("{} is a draft or prerelease", release.tag_name));
    }
    let Some(version) = version_of_tag(&release.tag_name) else {
        return Candidate::None(format!("{} is not an agent release tag", release.tag_name));
    };
    if !is_upgrade(current, &version) {
        return Candidate::None(format!("{version} is not newer than {current}"));
    }
    Candidate::Upgrade(version)
}

/// The release's page, built from the tag rather than taken from the JSON.
pub fn notes_url(release: &Release) -> String {
    format!("{RELEASE_PAGE_PREFIX}{}", release.tag_name)
}

/// The asset `name`, provided its URL is exactly where this release's `name` lives.
pub fn asset<'a>(release: &'a Release, name: &str) -> Result<&'a Asset, String> {
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == name)
        .ok_or_else(|| format!("{} has no asset {name}", release.tag_name))?;
    let expected = format!("{DOWNLOAD_PREFIX}{}/{name}", release.tag_name);
    if asset.browser_download_url != expected {
        return Err(format!(
            "{name} points at {}, not {expected}",
            asset.browser_download_url
        ));
    }
    Ok(asset)
}

/// Whether a request may go to `url`: HTTPS, to one of `ALLOWED_HOSTS`, on the default port,
/// with no credentials in it.
pub fn url_allowed(url: &reqwest::Url) -> Result<(), String> {
    if url.scheme() != "https" {
        return Err(format!("{url} is not HTTPS"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(format!("{url} carries credentials"));
    }
    if url.port().is_some() {
        return Err(format!("{url} names a port"));
    }
    match url.host_str() {
        Some(host) if ALLOWED_HOSTS.contains(&host) => Ok(()),
        _ => Err(format!("{url} is not a GitHub release host")),
    }
}

#[cfg(test)]
#[path = "release_tests.rs"]
mod tests;
