//! Native notifications (multi-window mw6): what the agent raises for each
//! signed-in account, with no window open, and to whom.
//!
//! - decide.rs: what a notice says, or that none is due (pure).
//! - hub.rs: the notice plane's subscribers, the windows' focus, and the
//!   `Notifier` seam the OS backends plug into.
//! - raise.rs: the agent's side: what happened and the account as it stands.
//! - heard.rs: telling the windows whether anything shows the notices.

pub(crate) mod decide;
mod heard;
pub(crate) mod hub;
mod raise;

pub(crate) use hub::NoticeHub;
pub use hub::NoticeToken;

impl<T, R: citadel_sdk::prelude::Ratchet> crate::kernel::CitadelWorkspaceService<T, R> {
    /// Open the notice plane to the native app that started the agent with
    /// `token`. Without it, no connection can subscribe.
    pub fn with_notice_token(mut self, token: NoticeToken) -> Self {
        let clients = self.tx_to_localhost_clients.clone();
        self.notices = std::sync::Arc::new(NoticeHub::new(Some(token), clients, Vec::new()));
        self
    }
}
