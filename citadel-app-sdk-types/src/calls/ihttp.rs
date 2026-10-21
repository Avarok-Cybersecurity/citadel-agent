//! `ihttp`: internal HTTP over authenticated Citadel sessions (R4.4), frame by frame. Identity is
//! the session's: [`Caller`] is stamped by the session or the kernel, never by an app. Frame
//! variants keep R4.4's names verbatim (`{RequestHead: ..}`, `"End"`, `{Credit: {n}}`); these
//! are the shapes `citadel-ihttp-types` must re-export rather than restate.

use super::HttpMethod;
use crate::Bytes;
use alloc::string::String;
use alloc::vec::Vec;

/// `(name, value)` pairs, in order.
pub type Headers = Vec<(String, String)>;

abi_types! {
    /// `ihttp://<app_id>@user:<username>/...` or `ihttp://<app_id>@scope:<scope_key>/...`.
    #[serde(rename_all = "snake_case")]
    pub enum Address {
        User { app_id: String, username: String },
        Scope { app_id: String, scope_key: String },
    }

    /// A method and a path pattern (`/a/:id/*`) the app serves.
    pub struct Route {
        pub method: HttpMethod,
        pub path: String,
    }

    #[serde(rename_all = "snake_case")]
    pub enum Via {
        P2p,
        Relay,
    }

    /// The other end of a stream, as its session vouches for it.
    pub struct Caller {
        pub username: String,
        pub cid: u64,
        pub app_id: String,
        pub install_id: String,
        pub via: Via,
    }

    pub struct RequestHead {
        pub method: HttpMethod,
        pub path: String,
        pub query: String,
        pub headers: Headers,
        /// None: not known up front (a streamed body).
        pub body_len: Option<u64>,
        pub app_id: String,
        pub scope_key: String,
    }

    pub struct ResponseHead {
        pub status: u16,
        pub headers: Headers,
        pub body_len: Option<u64>,
    }

    pub enum FrameKind {
        RequestHead(RequestHead),
        ResponseHead(ResponseHead),
        Body { bytes: Bytes },
        End,
        Cancel { reason: String },
        Credit { n: u32 },
    }

    pub struct IhttpFrame {
        pub v: u8,
        pub stream: u64,
        pub kind: FrameKind,
    }

    /// Advertise these routes under the app's own id.
    pub struct IhttpServe {
        pub routes: Vec<Route>,
    }

    /// A frame back to `peer` on a stream this instance serves.
    pub struct IhttpRespond {
        pub peer: Caller,
        pub frame: IhttpFrame,
    }

    /// A client frame to the server at `to`.
    pub struct IhttpCall {
        pub to: Address,
        pub frame: IhttpFrame,
    }

    /// The app has read one inbound body chunk of `stream`: the host grants the caller one more
    /// credit.
    pub struct IhttpConsumed {
        pub peer: Caller,
        pub stream: u64,
    }
}

impl IhttpFrame {
    pub fn new(stream: u64, kind: FrameKind) -> IhttpFrame {
        IhttpFrame { v: 1, stream, kind }
    }
}
