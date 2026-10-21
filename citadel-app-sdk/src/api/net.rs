//! `net`: HTTPS to the manifest's explicit hosts, through the member's agent (`net.host`) or
//! the workspace server (`net.server`). The host applies the egress policy; a denial is
//! `Refused::Policy`, before any connection.

use crate::{call, Pending};
use citadel_app_sdk_types::calls::*;

pub fn host_fetch(request: HttpRequest) -> Pending<HttpResponse> {
    call(&HostFetch { request })
}

pub fn server_fetch(request: HttpRequest) -> Pending<HttpResponse> {
    call(&ServerFetch { request })
}

/// A streamed response from a host fetch.
#[derive(Debug)]
pub struct NetStream {
    pub head: StreamHead,
}

pub fn host_fetch_stream(request: HttpRequest) -> Pending<NetStream> {
    call(&HostFetchOpen { request }).map(|head| NetStream { head })
}

impl NetStream {
    /// Up to `max` more bytes; `end` once the body is complete.
    pub fn read(&self, max: u32) -> Pending<StreamChunk> {
        call(&NetStreamRead {
            stream: self.head.stream,
            max,
        })
    }

    pub fn close(self) -> Pending<()> {
        call(&NetStreamClose {
            stream: self.head.stream,
        })
    }
}
