//! The updater: check, prepare, announce, install. Decisions are the pure modules'; this only
//! sequences them over the seams in io.rs.

use super::fetch::Refusal;
use super::io::{Fetched, Io};
use super::platform::Plan;
use super::policy::{decide, Decision, Situation, Trigger};
use super::release::{self, Candidate, Release};
use citadel_sdk::logging::{error, info, warn};
use parking_lot::Mutex;
use semver::Version;
use std::path::PathBuf;

pub const LOG_TARGET: &str = "citadel::update";
/// "Automatically install updates when no account is signed in" before anyone sets it: on, as
/// the owner asked (2026-10-02).
pub const AUTO_INSTALL_UNSET: bool = true;

/// A release newer than this agent, and how far it got.
#[derive(Debug, Clone)]
pub(super) struct Offer {
    pub version: Version,
    pub notes_url: String,
    pub download_url: String,
    /// Downloaded, verified and staged.
    pub staged: Option<PathBuf>,
    /// Nothing more to do for this version until a newer one: staged, refused, or link-only.
    /// False after a transient failure, so the next check tries again.
    pub settled: bool,
}

#[derive(Default)]
pub(super) struct State {
    pub(super) etag: Option<String>,
    pub(super) release: Option<Release>,
    pub(super) offer: Option<Offer>,
    pub(super) auto_install: Option<bool>,
    pub(super) last_checked: Option<u64>,
    pub(super) last_error: Option<String>,
}

pub struct Engine {
    pub(super) current: Version,
    pub(super) plan: Plan,
    pub(super) io: Io,
    pub(super) state: Mutex<State>,
    /// One check or install at a time; status reads never wait for it.
    busy: tokio::sync::Mutex<()>,
}

impl Engine {
    pub fn new(current: Version, plan: Plan, io: Io) -> Self {
        Self {
            current,
            plan,
            io,
            state: Mutex::default(),
            busy: tokio::sync::Mutex::new(()),
        }
    }

    pub async fn load_settings(&self) {
        let auto = match self.io.settings.auto_install().await {
            Ok(value) => value.unwrap_or(AUTO_INSTALL_UNSET),
            Err(e) => {
                warn!(target: LOG_TARGET, "the auto-install setting is unreadable, using {AUTO_INSTALL_UNSET}: {e}");
                AUTO_INSTALL_UNSET
            }
        };
        self.state.lock().auto_install = Some(auto);
    }

    /// Look for a newer release and, when there is one, prepare it and tell everyone.
    pub async fn check(&self) {
        let _busy = self.busy.lock().await;
        let etag = self.state.lock().etag.clone();
        let fetched = self.io.source.latest(etag).await;
        let now = (self.io.now)();
        let release = {
            let mut state = self.state.lock();
            state.last_checked = Some(now);
            match fetched {
                Err(e) => {
                    warn!(target: LOG_TARGET, "the release check failed: {e}");
                    state.last_error = Some(format!("The update check failed: {e}"));
                    return;
                }
                Ok(Fetched::NotModified) => state.release.clone(),
                Ok(Fetched::Release { body, etag }) => match release::parse(&body) {
                    Ok(release) => {
                        state.etag = etag;
                        state.release = Some(release.clone());
                        Some(release)
                    }
                    Err(e) => {
                        error!(target: LOG_TARGET, "{e}");
                        state.last_error = Some(e);
                        return;
                    }
                },
            }
        };
        let Some(release) = release else { return };
        let version = match release::candidate(&release, &self.current) {
            Candidate::None(why) => {
                info!(target: LOG_TARGET, "no update: {why}");
                let mut state = self.state.lock();
                state.offer = None;
                state.last_error = None;
                return;
            }
            Candidate::Upgrade(version) => version,
        };
        let known = self.state.lock().offer.clone();
        if let Some(offer) = known.filter(|o| o.version == version && o.settled) {
            self.io.announcer.announce(&self.available(&offer));
            return;
        }
        let offer = self.prepare(&release, version).await;
        self.state.lock().offer = Some(offer.clone());
        self.io.announcer.announce(&self.available(&offer));
    }

    async fn prepare(&self, release: &Release, version: Version) -> Offer {
        let notes_url = release::notes_url(release);
        let link = |asset: Option<&str>| {
            asset
                .and_then(|name| release::asset(release, name).ok())
                .map(|a| a.browser_download_url.clone())
                .unwrap_or_else(|| notes_url.clone())
        };
        let offer = |download_url, staged, settled| Offer {
            version: version.clone(),
            notes_url: notes_url.clone(),
            download_url,
            staged,
            settled,
        };
        let (asset, method) = match &self.plan {
            Plan::LinkOnly { asset, why } => {
                info!(target: LOG_TARGET, "{version} is available; offering the download: {why}");
                return offer(link(*asset), None, true);
            }
            Plan::Apply { asset, method } => (*asset, *method),
        };
        match self
            .fetch_and_verify(release, &version, asset, method)
            .await
        {
            Ok(staged) => {
                info!(target: LOG_TARGET, "{version} is downloaded and verified");
                self.state.lock().last_error = None;
                offer(link(Some(asset)), Some(staged), true)
            }
            Err(refusal) => {
                self.io.staging.discard(&version);
                let (why, url, settled) = match refusal {
                    Refusal::Transient(why) => (why, link(Some(asset)), false),
                    Refusal::LinkOnly(why) => (why, link(Some(asset)), true),
                    Refusal::Refused(why) => (why, notes_url.clone(), true),
                };
                error!(target: LOG_TARGET, "{version} was not installed, and the current agent is untouched: {why}");
                self.state.lock().last_error = Some(why);
                offer(url, None, settled)
            }
        }
    }

    /// Install the staged release if `trigger` and the situation call for it.
    pub async fn consider(&self, trigger: Trigger) -> Result<(), String> {
        let _busy = self.busy.lock().await;
        let (offer, auto_install) = {
            let state = self.state.lock();
            (
                state.offer.clone(),
                state.auto_install.unwrap_or(AUTO_INSTALL_UNSET),
            )
        };
        let staged = offer
            .as_ref()
            .and_then(|o| o.staged.clone().map(|p| (o, p)));
        let failed_before = staged
            .as_ref()
            .is_some_and(|(o, _)| self.io.staging.failed(&o.version));
        let situation = Situation {
            trigger,
            ready: staged.is_some() && self.io.installer.can_install().is_ok(),
            signed_in: self.io.sessions.signed_in(),
            auto_install,
            failed_before,
        };
        match (decide(&situation), staged) {
            (Decision::Install, Some((offer, path))) => {
                info!(target: LOG_TARGET, "installing {} ({trigger:?})", offer.version);
                self.io.installer.install(&path, &offer.version).map_err(|e| {
                    error!(target: LOG_TARGET, "installing {} failed; the current agent is untouched: {e}", offer.version);
                    self.state.lock().last_error = Some(e.clone());
                    e
                })
            }
            (Decision::Wait(why), _) if trigger == Trigger::UserAsked => {
                let why = self
                    .io
                    .installer
                    .can_install()
                    .err()
                    .unwrap_or(why.to_string());
                Err(format!("Nothing was installed: {why}"))
            }
            _ => Ok(()),
        }
    }
}
