//! Moves each opted-in session's ring to its workspace server, and learns from that server
//! whether the session may upload.
//!
//! The server is the single source of truth for consent. The agent asks it when a session
//! starts (`consent_query`) and reads every consent answer the server sends that session
//! afterwards: the answers to a window's `GetMyConsent`, `SetTelemetryConsent` and
//! `AcceptTerms` all pass through the agent on their way to the window. No window has to be
//! open for uploads to run, and none can switch them on by itself.
//!
//! The wire format belongs to the workspace protocol, which this crate does not depend on,
//! so the binary that ships supplies it as a `TelemetryCodec`.

mod kernel;

pub(crate) use kernel::run;

use super::browser::BrowserLimits;
use super::collector::TelemetryCollector;
use super::entry::TelemetryEntry;
use super::layer::now_ms;
use citadel_internal_service_types::BrowserConsoleEntry;
use citadel_sdk::logging::warn;
use futures::future::BoxFuture;
use std::sync::Arc;
use std::time::Duration;

const LOG_TARGET: &str = "citadel::telemetry";

/// What a frame the server sent a session means for telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerSignal {
    /// Not a telemetry answer: forwarded to the session's windows untouched.
    Unrelated,
    /// The session's consent as the server holds it: whether an upload would be stored, and
    /// whether the server stores metric entries (one built before metrics does not).
    Consent {
        uploads_allowed: bool,
        takes_metrics: bool,
    },
    /// An upload was stored.
    UploadStored,
    /// An upload was refused: consent is off or the terms changed since it was given.
    UploadRefused,
}

/// The workspace protocol's encoding of telemetry, supplied by the shipped binary.
pub trait TelemetryCodec: Send + Sync + 'static {
    /// The request asking the server for this session's consent.
    fn consent_query(&self) -> Result<Vec<u8>, String>;
    /// Upload frames carrying `entries`, split so each one is within the server's limits.
    fn upload_frames(&self, entries: Vec<TelemetryEntry>) -> Result<Vec<Vec<u8>>, String>;
    fn classify(&self, frame: &[u8]) -> ServerSignal;
}

/// Sends one frame to a session's server over its C2S link.
pub trait SessionLink: Send + Sync {
    fn send_to_server(&self, cid: u64, frame: Vec<u8>) -> BoxFuture<'static, Result<(), String>>;
}

pub struct TelemetryUplink {
    collector: Arc<TelemetryCollector>,
    codec: Arc<dyn TelemetryCodec>,
    flush_interval: Duration,
    /// `None` until the shipped binary configures it: then windows are told they may send.
    browser: Option<BrowserLimits>,
}

impl TelemetryUplink {
    /// `collector` must be the one the installed `TelemetryLayer` feeds. `flush_interval`
    /// is how often each opted-in session's ring is uploaded; it has no default.
    pub fn new(
        collector: Arc<TelemetryCollector>,
        codec: Arc<dyn TelemetryCodec>,
        flush_interval: Duration,
    ) -> Result<Self, String> {
        if flush_interval.is_zero() {
            return Err("the telemetry flush interval must be positive".into());
        }
        Ok(Self {
            collector,
            codec,
            flush_interval,
            browser: None,
        })
    }

    /// Take windows' console lines into their sessions' rings, within `limits`.
    pub fn with_browser_console(mut self, limits: BrowserLimits) -> Self {
        self.browser = Some(limits);
        self
    }

    /// Whether the greeting tells windows they may send console lines.
    pub fn takes_browser_console(&self) -> bool {
        self.browser.is_some()
    }

    /// A window's console lines for `cid`, stamped now; how many were kept.
    pub fn browser_console(&self, cid: u64, entries: &[BrowserConsoleEntry]) -> usize {
        self.browser.as_ref().map_or(0, |limits| {
            self.collector
                .capture_browser(cid, now_ms(), entries, limits)
        })
    }

    pub fn collector(&self) -> &Arc<TelemetryCollector> {
        &self.collector
    }

    pub fn flush_interval(&self) -> Duration {
        self.flush_interval
    }

    /// Asks the server for `cid`'s consent. Until it answers, the session captures nothing.
    pub async fn ask_consent(&self, link: &dyn SessionLink, cid: u64) {
        let sent = match self.codec.consent_query() {
            Ok(frame) => link.send_to_server(cid, frame).await,
            Err(err) => Err(err),
        };
        if let Err(err) = sent {
            warn!(target: LOG_TARGET, "Could not ask the server for session {cid}'s telemetry consent: {err}");
        }
    }

    /// Applies what the server just sent `cid`; returns whether the frame goes on to the
    /// session's windows. Upload answers answer the agent, so they stop here.
    pub fn on_server_frame(&self, cid: u64, frame: &[u8]) -> bool {
        let (applied, forward) = match self.codec.classify(frame) {
            ServerSignal::Unrelated => return true,
            ServerSignal::Consent {
                uploads_allowed: true,
                takes_metrics,
            } => (self.collector.opt_in(cid, takes_metrics), true),
            ServerSignal::Consent {
                uploads_allowed: false,
                ..
            } => (self.collector.opt_out(cid), true),
            ServerSignal::UploadStored => return false,
            // Stop, and discard what is held, until a consent answer allows uploads again.
            ServerSignal::UploadRefused => (self.collector.opt_out(cid), false),
        };
        if let Err(err) = applied {
            warn!(target: LOG_TARGET, "Telemetry consent for session {cid} not applied: {err}");
        }
        forward
    }

    /// Drains every opted-in session and sends its frames on its own link. A frame that
    /// fails to send is dropped with the rest of that drain: nothing is retried or kept.
    pub async fn flush(&self, link: &dyn SessionLink) {
        for cid in self.collector.opted_in_sessions() {
            let frames = self
                .collector
                .drain(cid)
                .map_err(|err| err.to_string())
                .and_then(|entries| match entries.is_empty() {
                    true => Ok(Vec::new()),
                    false => self.codec.upload_frames(entries),
                });
            let frames = match frames {
                Ok(frames) => frames,
                Err(err) => {
                    warn!(target: LOG_TARGET, "Telemetry for session {cid} dropped: {err}");
                    continue;
                }
            };
            for frame in frames {
                if let Err(err) = link.send_to_server(cid, frame).await {
                    warn!(target: LOG_TARGET, "Telemetry upload for session {cid} dropped: {err}");
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
