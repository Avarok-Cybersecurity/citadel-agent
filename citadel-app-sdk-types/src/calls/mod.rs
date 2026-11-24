//! The argument and reply structs of every op, by group. The op table itself is [`crate::ops`].

mod canvas;
mod files;
mod ihttp;
mod misc;
mod net;
mod nodes;
mod storage;
pub mod topic;
mod ui;

pub mod canvas_check;
pub mod session;

pub use canvas::*;
pub use files::*;
pub use ihttp::*;
pub use misc::*;
pub use net::*;
pub use nodes::*;
pub use storage::*;
pub use topic::{TopicClose, TopicEvent, TopicOpen, TopicOpened, TopicPublish};
pub use ui::*;

pub use canvas_check::{CanvasDrawError, LIMIT_CANVAS_DRAW};
pub use session::{
    SessionClose, SessionEvent, SessionOpen, SessionSend, SessionState, SessionStatus,
};

use alloc::string::String;

abi_types! {
    /// A reply that is one string.
    pub struct Text {
        pub text: String,
    }

    /// A reply that is opaque bytes.
    pub struct BytesValue {
        pub bytes: crate::Bytes,
    }
}
