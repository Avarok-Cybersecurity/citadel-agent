//! What the host delivers through `app_event`, one CBOR [`AppEvent`] per call.

use crate::calls::{Caller, IhttpFrame, SessionEvent, StorageBackend, TopicEvent};
use crate::lifecycle::SuspendReason;
use crate::Bytes;
use alloc::string::String;

abi_types! {
    #[serde(rename_all = "snake_case")]
    pub enum PointerKind {
        Down,
        Up,
        Move,
        Enter,
        Leave,
        Cancel,
    }

    #[serde(rename_all = "snake_case")]
    pub enum PointerType {
        Mouse,
        Pen,
        Touch,
    }

    pub struct Modifiers {
        pub shift: bool,
        pub ctrl: bool,
        pub alt: bool,
        pub meta: bool,
    }

    /// Coordinates are CSS pixels relative to the app's canvas; nothing about the page.
    pub struct PointerEvent {
        pub kind: PointerKind,
        pub pointer_type: PointerType,
        pub x: f32,
        pub y: f32,
        pub button: i16,
        pub buttons: u16,
        pub modifiers: Modifiers,
    }

    pub struct KeyEvent {
        pub down: bool,
        pub key: String,
        pub code: String,
        pub repeat: bool,
        pub modifiers: Modifiers,
    }

    pub struct WheelEvent {
        pub x: f32,
        pub y: f32,
        pub dx: f32,
        pub dy: f32,
        pub modifiers: Modifiers,
    }

    #[serde(rename_all = "snake_case")]
    pub enum AppEvent {
        /// The first event after `app_init` answered OK: the start (`Initialised -> Running`).
        Init,
        /// The surface is out of sight: frames and input stop until `Resume`. Always delivered.
        Suspend { reason: SuspendReason },
        /// The surface is back. Always delivered.
        Resume,
        Resize { width: u32, height: u32, dpr: f32 },
        Pointer(PointerEvent),
        Key(KeyEvent),
        Wheel(WheelEvent),
        Focus { focused: bool },
        Visibility { visible: bool },
        NodeChanged { node_id: String },
        StorageChanged { backend: StorageBackend, key: String },
        /// A toolbar item, menu item or dialog action the user chose.
        UiAction { id: String },
        DeepLink { query: String },
        /// For hosts that drive frames as events; the SDK treats it like `app_frame`.
        Frame { dt_ms: u32 },
        /// A pending reply is ready to read (see `abi`).
        ReplyReady { handle: i32 },
        /// An internal-HTTP frame: a request for a route the app serves, or the response on a
        /// stream it opened.
        Ihttp { from: Caller, frame: IhttpFrame },
        /// Something happened on topic `name`, which the app opened.
        Topic { name: String, event: TopicEvent },
        /// A file the member chose to open in this app, which its manifest's `opens` lists: its
        /// name, its MIME type (empty when none was recorded) and its bytes, read on this device
        /// and at most `opens.max_bytes`. Delivered to an instance on `opens.surface`.
        OpenFile { name: String, mime: String, bytes: Bytes },
        /// Something happened on `session`, which the app opened (`citadel.session`).
        Session { session: u32, event: SessionEvent },
        /// The user activated the canvas `Region` `id` (keyboard or pointer). ABI minor 1.
        Activate { id: String },
    }
}
