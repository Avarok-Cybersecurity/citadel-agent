//! `net`: egress through the member's agent (`net.host`) or the workspace server (`net.server`),
//! to the manifest's explicit hosts only. The host decides; these are only requests.

use super::Headers;
use crate::Bytes;
use alloc::string::String;

abi_types! {
    #[serde(rename_all = "SCREAMING_SNAKE_CASE")]
    pub enum HttpMethod {
        Get,
        Post,
        Put,
        Delete,
        Patch,
        Head,
    }

    pub struct HttpRequest {
        pub method: HttpMethod,
        pub url: String,
        pub headers: Headers,
        pub body: Bytes,
    }

    pub struct HttpResponse {
        pub status: u16,
        pub headers: Headers,
        pub body: Bytes,
    }

    pub struct HostFetch {
        pub request: HttpRequest,
    }

    pub struct ServerFetch {
        pub request: HttpRequest,
    }

    /// Open a streamed host fetch; read it with `NetStreamRead`.
    pub struct HostFetchOpen {
        pub request: HttpRequest,
    }

    pub struct StreamHead {
        pub stream: u32,
        pub status: u16,
        pub headers: Headers,
    }

    /// Up to `max` bytes of an open stream.
    pub struct NetStreamRead {
        pub stream: u32,
        pub max: u32,
    }

    pub struct StreamChunk {
        pub bytes: Bytes,
        pub end: bool,
    }

    pub struct NetStreamClose {
        pub stream: u32,
    }
}
