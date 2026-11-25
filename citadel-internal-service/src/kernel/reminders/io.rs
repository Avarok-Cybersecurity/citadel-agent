//! The reminder runner's edge on the agent: the account's own store, under a reserved `agent:`
//! key no window may read or write (requests/local_db), and who shows a fired reminder.

use super::runner::{Delivery, ReminderIo};
use crate::kernel::conversations::kv::{ConversationKv, HostKv};
use crate::kernel::conversations::ConversationIo;
use crate::kernel::ilm::HostIo;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    AppNotice, AppNoticeNotification, InternalServiceResponse, NativeNotice, NoticeKind,
    NoticeTarget,
};
use citadel_sdk::prelude::Ratchet;
use futures::future::BoxFuture;
use std::sync::Arc;

/// In the account's own bucket. `agent:` keys are the agent's alone (requests/local_db).
pub(crate) const REMINDERS_KEY: &str = "agent:app_reminders";

impl<T: IOInterface + Sync, R: Ratchet> CitadelWorkspaceService<T, R> {
    fn account_kv(&self, cid: u64) -> HostKv {
        let io: Arc<dyn HostIo> = Arc::new(self.clone());
        HostKv::account(io, cid)
    }

    /// The menu-bar app's notice for `notice`, if the account is signed in here.
    fn native_app_notice(&self, cid: u64, notice: &AppNotice, late: bool) -> Option<NativeNotice> {
        let (account, server_host) = {
            let map = self.server_connection_map.read();
            let conn = map.get(&cid)?;
            (conn.username.clone(), conn.server_host.clone())
        };
        Some(NativeNotice {
            cid,
            kind: NoticeKind::App,
            title: if late {
                format!("Missed: {}", notice.title)
            } else {
                notice.title.clone()
            },
            body: notice.body.clone(),
            target: NoticeTarget {
                account,
                server_host,
                open: format!("node:{}", notice.node_id),
            },
            id: Some(notice.id.clone()),
            request_id: None,
        })
    }

    /// Show `notice` through the menu-bar app, unless nothing listens or the account is muted
    /// there; whether it was shown.
    pub(crate) async fn show_app_notice(&self, cid: u64, notice: &AppNotice, late: bool) -> bool {
        if !self.notices.is_heard() || self.is_muted(cid).await {
            return false;
        }
        match self.native_app_notice(cid, notice, late) {
            Some(native) => {
                self.notices.send_notice(&native);
                true
            }
            None => false,
        }
    }
}

impl<T: IOInterface + Sync, R: Ratchet> ReminderIo for CitadelWorkspaceService<T, R> {
    fn load(&self, cid: u64) -> BoxFuture<'static, Result<Option<Vec<u8>>, String>> {
        let kv = self.account_kv(cid);
        Box::pin(async move { kv.get(REMINDERS_KEY).await })
    }

    fn save(&self, cid: u64, bytes: Vec<u8>) -> BoxFuture<'static, Result<(), String>> {
        let kv = self.account_kv(cid);
        Box::pin(async move { kv.set(REMINDERS_KEY, bytes).await })
    }

    fn deliver(
        &self,
        cid: u64,
        notice: AppNotice,
        late: bool,
        to_windows: bool,
    ) -> BoxFuture<'static, Delivery> {
        let this = self.clone();
        Box::pin(async move {
            let shown_natively = this.show_app_notice(cid, &notice, late).await;
            let told = to_windows
                && this.publish(
                    cid,
                    InternalServiceResponse::AppNoticeNotification(Box::new(
                        AppNoticeNotification {
                            cid,
                            notice,
                            late,
                            shown_natively,
                            request_id: None,
                        },
                    )),
                ) > 0;
            if shown_natively || told {
                Delivery::Shown
            } else {
                Delivery::Unreached
            }
        })
    }

    fn signed_in(&self, cid: u64) -> bool {
        self.server_connection_map.read().contains_key(&cid)
    }
}
