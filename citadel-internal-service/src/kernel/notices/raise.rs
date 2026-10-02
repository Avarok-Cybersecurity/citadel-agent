//! The agent's side of native notices: what happened, read from the
//! notifications it already sends its windows; the account as it stands now;
//! the decision (decide.rs); and the account rows the menu-bar app lists.

use super::decide::{decide, NoticeContext, NoticeSource};
use crate::kernel::conversations::engine::{preferences, store};
use crate::kernel::conversations::kv::KvResult;
use crate::kernel::conversations::ConversationIo;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    AccountPreferences, AccountRow, InternalServiceResponse, NoticeRows,
};
use citadel_sdk::logging::warn;
use citadel_sdk::prelude::Ratchet;

const MUTED_KEY: &str = "agent_account_muted_";

/// What, among the notifications a session's windows are sent, deserves a
/// notice. Messages and calls are raised by the conversation store, which
/// reads them before any window does.
pub(crate) fn source_of(response: &InternalServiceResponse) -> Option<(u64, NoticeSource)> {
    match response {
        InternalServiceResponse::PeerRegisterNotification(n) => Some((
            n.cid,
            NoticeSource::PeerRequest {
                peer: n.peer_cid,
                peer_username: Some(n.peer_username.clone()),
            },
        )),
        InternalServiceResponse::GroupInviteNotification(n) => Some((
            n.cid,
            NoticeSource::GroupInvite {
                peer: n.peer_cid,
                peer_username: None,
            },
        )),
        InternalServiceResponse::FileTransferRequestNotification(n) => Some((
            n.cid,
            NoticeSource::FileOffer {
                peer: n.peer_cid,
                peer_username: None,
                file_name: n.metadata.name.clone(),
            },
        )),
        _ => None,
    }
}

impl<T: IOInterface + Sync, R: Ratchet> CitadelWorkspaceService<T, R> {
    /// `response` is going to `cid`'s windows; raise a notice if it calls for one.
    pub(crate) fn notice_for(&self, response: &InternalServiceResponse) {
        if let Some((cid, source)) = source_of(response) {
            self.raise_notice(cid, source);
        }
    }

    /// Decide and send a notice for `source`, off the caller's path: the
    /// caller is mid-delivery, often under a lock.
    pub(crate) fn raise_notice(&self, cid: u64, source: NoticeSource) {
        if !self.notices.is_heard() {
            return;
        }
        // What the user had in front of them when it happened, not later.
        let focused = self.notices.focused_on(cid);
        let this = self.clone();
        tokio::task::spawn(async move {
            let source = this.named(cid, source);
            if let Some(ctx) = this.notice_context(cid, focused).await {
                if let Some(notice) = decide(&source, &ctx) {
                    this.notices.send_notice(&notice);
                }
            }
        });
    }

    /// The account rows changed (a message, a read, a mute): send them again.
    pub(crate) fn rows_changed(&self) {
        if !self.notices.is_heard() {
            return;
        }
        let this = self.clone();
        tokio::task::spawn(async move {
            let rows = NoticeRows {
                cid: 0,
                rows: this.notice_rows().await,
                request_id: None,
            };
            this.notices.send_rows(&rows);
        });
    }

    pub(crate) async fn notice_rows(&self) -> Vec<AccountRow> {
        let accounts: Vec<(u64, String, Option<String>)> = self
            .server_connection_map
            .read()
            .iter()
            .map(|(cid, conn)| (*cid, conn.username.clone(), conn.server_host.clone()))
            .collect();
        let mut rows = Vec::with_capacity(accounts.len());
        for (cid, username, server_host) in accounts {
            let unread = match store(self, cid).list().await {
                Ok(list) => list.iter().map(|m| m.unread_count.max(0.0) as u32).sum(),
                Err(e) => {
                    warn!(target: "citadel", "[NOTICES] {cid}: unread count unreadable: {e}");
                    0
                }
            };
            rows.push(AccountRow {
                cid,
                username,
                server_host,
                unread,
                muted: self.is_muted(cid).await,
            });
        }
        rows
    }

    pub(crate) async fn set_muted(&self, cid: u64, muted: bool) -> KvResult<()> {
        self.kv()
            .set(&format!("{MUTED_KEY}{cid}"), vec![u8::from(muted)])
            .await
    }

    async fn is_muted(&self, cid: u64) -> bool {
        match self.kv().get(&format!("{MUTED_KEY}{cid}")).await {
            Ok(value) => value.is_some_and(|v| v.first() == Some(&1)),
            Err(e) => {
                warn!(target: "citadel", "[NOTICES] {cid}: mute unreadable, treated as unmuted: {e}");
                false
            }
        }
    }

    /// Fill in the peer's name where the notification did not carry it.
    fn named(&self, cid: u64, source: NoticeSource) -> NoticeSource {
        let name = |peer: u64| self.peer_username(cid, peer);
        match source {
            NoticeSource::GroupInvite {
                peer,
                peer_username: None,
            } => NoticeSource::GroupInvite {
                peer,
                peer_username: name(peer),
            },
            NoticeSource::FileOffer {
                peer,
                peer_username: None,
                file_name,
            } => NoticeSource::FileOffer {
                peer,
                peer_username: name(peer),
                file_name,
            },
            other => other,
        }
    }

    async fn notice_context(&self, cid: u64, focused: Vec<Option<u64>>) -> Option<NoticeContext> {
        let (account, server_host) = {
            let map = self.server_connection_map.read();
            let conn = map.get(&cid)?;
            (conn.username.clone(), conn.server_host.clone())
        };
        // Unreadable preferences show the least: the defaults hide previews.
        let prefs = preferences(self, cid).await.unwrap_or_else(|e| {
            warn!(target: "citadel", "[NOTICES] {cid}: preferences unreadable, using the defaults: {e}");
            AccountPreferences::UI_DEFAULTS
        });
        Some(NoticeContext {
            cid,
            account,
            server_host,
            preview: prefs.notification_preview,
            muted: self.is_muted(cid).await,
            focused,
        })
    }
}

#[cfg(test)]
#[path = "raise_tests.rs"]
mod tests;
