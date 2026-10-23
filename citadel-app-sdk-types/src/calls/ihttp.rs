//! `ihttp`: internal HTTP over authenticated Citadel sessions (R4.4), frame by frame. Identity is
//! the session's: [`Caller`] is stamped by the session or the kernel, never by an app. Frame
//! variants keep R4.4's names verbatim (`{RequestHead: ..}`, `"End"`, `{Credit: {n}}`); these
//! are the shapes `citadel-ihttp-types` must re-export rather than restate.

use super::HttpMethod;
use crate::{Bytes, Capability};
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

    /// How a stream reached its other end: a direct Citadel P2P session, the workspace server's
    /// relay, or (topics only) the app's Citadel group.
    #[derive(Copy, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum Via {
        P2p,
        Relay,
        Group,
    }

    /// How a client stream travels. There is no default: every client names one.
    /// `PreferP2pThenRelay` falls back to the relay only before the server accepted the
    /// `RequestHead`, and only when `ihttp.call` is also effective; a stream never switches.
    #[derive(Copy, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum Transport {
        P2p,
        Relay,
        PreferP2pThenRelay,
    }

    /// The planes a server advertises on. Both fields are required; at least one is true.
    #[derive(Copy, Eq)]
    pub struct ServeOn {
        pub relay: bool,
        pub p2p: bool,
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

    /// Advertise these routes under the app's own id, on the planes `on` names. The host
    /// advertises each requested plane the install holds effectively and answers `ungranted`
    /// when a requested plane is not held; the planes that are held stay advertised.
    pub struct IhttpServe {
        pub routes: Vec<Route>,
        pub on: ServeOn,
    }

    /// A frame back to `peer` on a stream this instance serves.
    pub struct IhttpRespond {
        pub peer: Caller,
        pub frame: IhttpFrame,
    }

    /// A client frame to the server at `to`, on `transport`. Every frame of one stream names
    /// the same transport.
    pub struct IhttpCall {
        pub to: Address,
        pub frame: IhttpFrame,
        pub transport: Transport,
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

impl Transport {
    pub const ALL: [Transport; 3] = [
        Transport::P2p,
        Transport::Relay,
        Transport::PreferP2pThenRelay,
    ];

    /// The capability the first attempt needs.
    pub const fn first_capability(self) -> Capability {
        match self {
            Transport::P2p | Transport::PreferP2pThenRelay => Capability::IhttpP2pCall,
            Transport::Relay => Capability::IhttpCall,
        }
    }

    /// The capability a fallback to the relay needs; `None`: this transport never falls back.
    pub const fn fallback_capability(self) -> Option<Capability> {
        match self {
            Transport::PreferP2pThenRelay => Some(Capability::IhttpCall),
            Transport::P2p | Transport::Relay => None,
        }
    }

    /// The serde tag (`"prefer_p2p_then_relay"`).
    pub const fn tag(self) -> &'static str {
        match self {
            Transport::P2p => "p2p",
            Transport::Relay => "relay",
            Transport::PreferP2pThenRelay => "prefer_p2p_then_relay",
        }
    }
}

impl Via {
    /// The capability serving on this plane needs; `None` for `Group`, which serves nothing.
    pub const fn serve_capability(self) -> Option<Capability> {
        match self {
            Via::P2p => Some(Capability::IhttpP2pServe),
            Via::Relay => Some(Capability::IhttpServe),
            Via::Group => None,
        }
    }
}

impl ServeOn {
    /// The planes requested, each with the capability it needs.
    pub fn planes(self) -> impl Iterator<Item = (Via, Capability)> {
        [(self.relay, Via::Relay), (self.p2p, Via::P2p)]
            .into_iter()
            .filter_map(|(on, via)| on.then_some(via))
            .filter_map(|via| via.serve_capability().map(|c| (via, c)))
    }

    /// Serving on no plane is not a request.
    pub const fn is_empty(self) -> bool {
        !self.relay && !self.p2p
    }
}
