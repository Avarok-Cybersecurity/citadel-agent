//! The small groups: `events`, `navigate`, `chat`, `members`, `clipboard`, `notify`,
//! `documents`, `log`.

use crate::Bytes;
use alloc::string::String;
use alloc::vec::Vec;

abi_types! {
    #[serde(rename_all = "snake_case")]
    pub enum EventKind {
        Pointer,
        Key,
        Wheel,
        Focus,
        Visibility,
    }

    /// Which input events the host forwards; none until asked.
    pub struct Subscribe {
        pub kinds: Vec<EventKind>,
    }

    /// Start or stop `app_frame` callbacks.
    pub struct RequestFrames {
        pub on: bool,
    }

    /// Open an internal link; the host classifies it and refuses anything external.
    pub struct NavigateOpen {
        pub href: String,
    }

    pub struct ChatMessage {
        pub id: String,
        pub author: String,
        pub text: String,
        pub sent_at_ms: u64,
    }

    pub struct ChatMessages {
        pub messages: Vec<ChatMessage>,
    }

    /// The hosting node's channel only.
    pub struct ChatRecent {
        pub limit: u32,
    }

    pub struct ChatSend {
        pub text: String,
    }

    pub struct Member {
        pub username: String,
        pub display_name: String,
        pub online: bool,
    }

    pub struct MemberList {
        pub members: Vec<Member>,
    }

    pub struct MembersList {}

    /// Host-mediated, behind a user gesture; reply `Text`.
    pub struct ClipboardReadText {}

    pub struct ClipboardWriteText {
        pub text: String,
    }

    /// A host toast badged with the app's name.
    pub struct Notify {
        pub title: String,
        pub body: String,
    }

    pub struct DocOpen {
        pub node_id: String,
    }

    pub struct DocState {
        pub state: Bytes,
    }

    pub struct DocApply {
        pub node_id: String,
        pub update: Bytes,
    }

    pub struct DocPull {
        pub node_id: String,
        pub since: Bytes,
    }

    pub struct DocUpdates {
        pub updates: Vec<Bytes>,
    }

    #[serde(rename_all = "snake_case")]
    pub enum LogLevel {
        Debug,
        Info,
        Warn,
        Error,
    }

    pub struct LogWrite {
        pub level: LogLevel,
        pub message: String,
    }
}
