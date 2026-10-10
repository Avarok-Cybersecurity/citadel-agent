//! Agent updates: what the agent tells every window and the menu-bar app about a newer release,
//! and what it hands the menu-bar app to install.
//!
//! None of these belongs to a session: `cid` is always 0. An update concerns the agent itself,
//! so every window hears it, signed in or not.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg(feature = "typescript")]
use ts_rs::TS;

/// A release newer than the running agent.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct UpdateAvailable {
    /// 0: an update is the agent's, not one session's.
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    /// The running version, `X.Y.Z`.
    pub current: String,
    /// The release's version, `X.Y.Z`.
    pub latest: String,
    /// The release page on GitHub.
    pub notes_url: String,
    /// What to download by hand when this agent cannot install it itself: the asset for this
    /// install, or the release page when none fits.
    pub download_url: String,
    /// Downloaded and verified: "Restart to update" installs it now. False: link out only.
    pub ready: bool,
    /// The download carried a valid ML-DSA-65 (post-quantum) signature by the release key, over
    /// this release's tag, the file's name and its sha256. Nothing is staged without it, and
    /// without it `ready` is false and `UpdateStatus::last_error` (prefixed "ML-DSA:") says why.
    pub mldsa_verified: bool,
    pub request_id: Option<Uuid>,
}

/// Where the updater stands, answered to every update request.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct UpdateStatus {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub current: String,
    /// The newer release, if one was found.
    pub available: Option<UpdateAvailable>,
    /// "Automatically install updates when no account is signed in".
    pub auto_install: bool,
    /// When the last check finished, in seconds since the Unix epoch.
    #[cfg_attr(feature = "typescript", ts(type = "bigint | null"))]
    pub last_checked: Option<u64>,
    /// Why the last check, download, verification or install did not succeed.
    pub last_error: Option<String>,
    pub request_id: Option<Uuid>,
}

/// To the menu-bar app only (it owns the bundle): install the verified disk image at `path`,
/// which holds version `version`, then relaunch.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct UpdateInstall {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub version: String,
    pub path: String,
    pub request_id: Option<Uuid>,
}
