//! The safe wrappers, one module per capability group. `canvas` and `log` answer
//! `Result<_, Refused>` at once; every other group answers a [`crate::Pending`].

pub mod canvas;
pub mod log;
pub mod misc;
pub mod net;
pub mod nodes;
pub mod storage;
pub mod ui;

pub use misc::{chat, clipboard, documents, events, files, members, navigate, notify};
pub use nodes::md;
