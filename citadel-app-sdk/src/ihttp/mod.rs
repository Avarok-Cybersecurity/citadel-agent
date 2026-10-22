//! `ihttp`: internal HTTP over Citadel sessions (R4.4), on top of the frame ops. A served request
//! is assembled from its frames (granting the caller credit chunk by chunk) and handed to the
//! handler with a [`ResponseWriter`]; a client call sends its request under the server's credit
//! and assembles the response. Who called is [`Caller`], stamped by the session or the kernel.

mod client;
mod outbox;
mod server;

pub use citadel_app_sdk_types::calls::{Address, Caller, Headers, ResponseHead, Route};
pub use client::ClientEvent;
pub use server::ResponseWriter;

use crate::abi::RawAbi;
use citadel_app_sdk_types::calls::{FrameKind, HttpMethod, IhttpFrame};
use std::any::Any;
use std::cell::RefCell;

/// A served request, body complete.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub caller: Caller,
    pub stream: u64,
    pub method: HttpMethod,
    pub path: String,
    pub query: String,
    pub headers: Headers,
    pub body: Vec<u8>,
}

/// A request to another app's server.
#[derive(Clone, Debug, PartialEq)]
pub struct Outbound {
    pub method: HttpMethod,
    pub path: String,
    pub query: String,
    pub headers: Headers,
    pub body: Vec<u8>,
}

/// A whole response.
#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    pub head: ResponseHead,
    pub body: Vec<u8>,
}

type Hook<R> = fn(&R, Caller, IhttpFrame, &mut dyn Any) -> Option<(Caller, IhttpFrame)>;

thread_local! {
    /// Set by the first `serve` or client call. Until then the runtime never reaches this module,
    /// so an app that does not use internal HTTP does not import the `ihttp` group at all.
    static HOOK: RefCell<Option<Box<dyn Any>>> = const { RefCell::new(None) };
}

pub(crate) fn install<R: RawAbi + 'static>() {
    HOOK.with(|h| {
        h.borrow_mut()
            .get_or_insert_with(|| Box::new(on_frame::<R> as Hook<R>));
    });
}

/// An inbound frame: the SDK's (`None`) or the app's to handle (given back).
pub(crate) fn route<R: RawAbi + 'static>(
    raw: &R,
    from: Caller,
    frame: IhttpFrame,
    app: &mut dyn Any,
) -> Option<(Caller, IhttpFrame)> {
    let hook = HOOK.with(|h| {
        h.borrow()
            .as_ref()
            .and_then(|b| b.downcast_ref::<Hook<R>>().copied())
    });
    match hook {
        Some(on_frame) => on_frame(raw, from, frame, app),
        None => Some((from, frame)),
    }
}

fn on_frame<R: RawAbi>(
    raw: &R,
    from: Caller,
    frame: IhttpFrame,
    app: &mut dyn Any,
) -> Option<(Caller, IhttpFrame)> {
    let unclaimed =
        if matches!(frame.kind, FrameKind::RequestHead(_)) || server::owns(&from, frame.stream) {
            server::on_frame(raw, from, frame, app)
        } else {
            Some((from, frame))
        };
    let (from, frame) = unclaimed?;
    client::on_frame(raw, from, frame, app)
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::*;
    use crate::ffi::WASM;
    use crate::Pending;
    use citadel_app_sdk_types::Refused;

    /// Advertise `routes` and hand every complete request to `handler`, with the app.
    pub fn serve<A: 'static>(
        routes: Vec<Route>,
        handler: impl FnMut(&mut A, Request, ResponseWriter) + 'static,
    ) -> Pending<()> {
        server::serve(&WASM, routes, handler)
    }

    impl ResponseWriter {
        /// The whole response; body chunks go out as the caller grants credit.
        pub fn respond(self, status: u16, headers: Headers, body: &[u8]) -> Result<(), Refused> {
            self.respond_with(&WASM, status, headers, body)
        }
    }

    /// Calls the internal HTTP server at one address.
    #[derive(Clone, Debug)]
    pub struct Client {
        pub to: Address,
    }

    impl Client {
        pub fn new(to: Address) -> Client {
            Client { to }
        }

        /// The response, whole, to `f` with the app.
        pub fn call<A: 'static>(
            &self,
            request: Outbound,
            f: impl FnOnce(&mut A, Result<Response, Refused>) + 'static,
        ) {
            client::call(&WASM, self.to.clone(), request, f)
        }

        /// The response as it streams, to `on` with the app.
        pub fn call_stream<A: 'static>(
            &self,
            request: Outbound,
            on: impl FnMut(&mut A, ClientEvent) + 'static,
        ) {
            client::call_stream(&WASM, self.to.clone(), request, on)
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use wasm::*;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
