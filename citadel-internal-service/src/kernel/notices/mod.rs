//! Native notifications (multi-window mw6): what the agent raises for each
//! signed-in account, with no window open, and to whom.
//!
//! - decide.rs: what a notice says, or that none is due (pure).
//! - hub.rs: the notice plane's subscribers, the windows' focus, and the
//!   `Notifier` seam the OS backends plug into.
//! - raise.rs: the agent's side: what happened and the account as it stands.

pub(crate) mod decide;
pub(crate) mod hub;
mod raise;

pub(crate) use hub::NoticeHub;
pub use hub::NoticeToken;
