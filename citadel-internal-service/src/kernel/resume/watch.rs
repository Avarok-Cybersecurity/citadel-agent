//! The watch on one session's resume credential: it waits for the moment the credential
//! expires and says so once, to the session's windows and as a native notice whose click
//! opens the account's sign-in.
//!
//! The clock is a port, so the wait is tested against a clock that sleeps, wakes late and is
//! set back (watch_tests.rs).

use super::expiry::{step, Step};
use crate::kernel::reconnect::Reauth;
use crate::kernel::session_route::SessionRoute;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceResponse, NativeNotice, NoticeKind, NoticeTarget, ResumeCredentialExpired,
};
use citadel_sdk::logging::info;
use citadel_sdk::prelude::{Ratchet, ResumeCredential};
use futures::future::BoxFuture;
use std::time::Duration;

/// This computer's clock, and a pause on it.
pub(crate) trait Clock: Send + Sync {
    /// Unix milliseconds.
    fn now_ms(&self) -> u64;
    fn sleep(&self, duration: Duration) -> BoxFuture<'_, ()>;
}

pub(crate) struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        super::wall_clock_ms()
    }

    fn sleep(&self, duration: Duration) -> BoxFuture<'_, ()> {
        Box::pin(tokio::time::sleep(duration))
    }
}

/// Waits until the credential expiring at `expires_at_ms` (this computer's clock) has
/// expired. `false` when `still_held` said, at a look at the clock, that the session no
/// longer holds it: there is then nothing to say.
pub(crate) async fn until_expired(
    clock: &dyn Clock,
    expires_at_ms: u64,
    still_held: impl Fn() -> bool,
) -> bool {
    loop {
        if !still_held() {
            return false;
        }
        match step(clock.now_ms(), expires_at_ms) {
            Step::Expired => return true,
            Step::Wait(duration) => clock.sleep(duration).await,
        }
    }
}

/// What a notice's target says to open for the account's sign-in itself: the page signs the
/// still-live session out and shows the form (the workspace's `open=sign-in` link target).
pub(crate) const OPEN_SIGN_IN: &str = "sign-in";

/// The native notice for an account whose resume credential has run out; a click opens the
/// account's sign-in.
pub(crate) fn sign_in_notice(
    cid: u64,
    account: String,
    server_host: Option<String>,
) -> NativeNotice {
    NativeNotice {
        cid,
        kind: NoticeKind::SignInNeeded,
        title: format!("Sign in to {account} again"),
        body: "The time this computer stays signed in has run out. \
               Sign in again to stay connected."
            .to_string(),
        target: NoticeTarget {
            account,
            server_host,
            open: OPEN_SIGN_IN.to_string(),
        },
        request_id: None,
    }
}

/// Whether `reauth` is still the credential `watched` (a credential re-issued after a change
/// to the account's sign-in is the same one: its times do not move).
fn same(reauth: &Reauth, watched: &ResumeCredential) -> bool {
    matches!(reauth, Reauth::Resume(held)
        if held.valid_until_utc() == watched.valid_until_utc()
            && held.issued_at_utc() == watched.issued_at_utc())
}

/// The moment `credential` expires by this computer's clock: the server's moment, moved by
/// how far this clock was off from the server's when the credential was accepted. The watch
/// waits for this, never for the server's time, so a wrong clock fires neither early nor late.
pub(crate) fn deadline(credential: &ResumeCredential) -> u64 {
    credential.expires_at_local()
}

/// Starts the watch for `cid`'s credential. It ends by itself when the session ends, signs in
/// again, or the credential expires.
pub(crate) fn spawn<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    watched: ResumeCredential,
) {
    let this = this.clone();
    drop(tokio::task::spawn(async move {
        let still_held = || {
            let lock = this.server_connection_map.read();
            lock.get(&cid)
                .is_some_and(|conn| same(&conn.reconnect.reauth, &watched))
        };
        if until_expired(&SystemClock, deadline(&watched), still_held).await {
            announce(&this, cid, watched.valid_until_utc());
        }
    }));
}

fn announce<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    valid_until_utc: u64,
) {
    let found = {
        let lock = this.server_connection_map.read();
        lock.get(&cid).map(|conn| {
            (
                conn.subscribers.clone(),
                conn.username.clone(),
                conn.server_host.clone(),
            )
        })
    };
    let Some((subscribers, username, server_host)) = found else {
        return;
    };
    info!(target: "citadel", "[Resume] {cid}: the resume credential has expired; the next drop ends in a sign-in");
    let expired = InternalServiceResponse::ResumeCredentialExpired(ResumeCredentialExpired {
        cid,
        valid_until_utc,
        request_id: None,
    });
    let _ = SessionRoute::new(subscribers, this.tx_to_localhost_clients.clone()).send(expired);
    if this.notices.is_heard() {
        this.notices
            .send_notice(&sign_in_notice(cid, username, server_host));
    }
    // The account rows carry the mark the menu-bar app draws.
    this.rows_changed();
}
