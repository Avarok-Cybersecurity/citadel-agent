//! `ihttp`: internal HTTP over Citadel sessions (R4.4), on top of the frame ops. A served request
//! is assembled from its frames (granting the caller credit chunk by chunk) and handed to the
//! handler with a [`ResponseWriter`]; a client call sends its request under the server's credit
//! and assembles the response. Who called is [`Caller`], stamped by the session or the kernel.

mod client;
mod client_whole;
mod fanout;
mod fanout_plan;
mod outbox;
mod server;

pub use citadel_app_sdk_types::calls::{
    Address, Caller, Headers, ResponseHead, Route, ServeOn, Transport, Via,
};
pub use client::ClientEvent;
pub(crate) use fanout::tick as fanout_tick;
pub use fanout::{FanoutHandle, FanoutLimits, Outcome};
pub use fanout_plan::{LIMIT_PER_CALL_MS, LIMIT_TOTAL_BYTES, POLICY_ZERO_LIMIT};
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

/// A whole response, and the plane that carried it.
#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    pub head: ResponseHead,
    pub via: Via,
    pub body: Vec<u8>,
}

/// A call in flight. Dropping it does not cancel; [`CallHandle::cancel`] does.
#[derive(Debug)]
#[must_use = "keep the handle to cancel the call, or drop it to let the call run"]
pub struct CallHandle {
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    key: client::Key,
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

    /// Advertise `routes` on the planes `on` names and hand every complete request to
    /// `handler`, with the app. Who called, and over which plane, is `Request::caller`.
    pub fn serve<A: 'static>(
        routes: Vec<Route>,
        on: ServeOn,
        handler: impl FnMut(&mut A, Request, ResponseWriter) + 'static,
    ) -> Pending<()> {
        server::serve(&WASM, routes, on, handler)
    }

    impl CallHandle {
        /// Give up: `Cancel { caller }` goes to the server; the call ends `Failed`.
        pub fn cancel(self) {
            client::cancel(&WASM, &self.key)
        }
    }

    impl FanoutHandle {
        /// End the fan-out: running calls are cancelled, and they and the waiting ones skipped.
        pub fn cancel(self) {
            fanout::cancel(self.id)
        }
    }

    /// Call every target with `request` on `transport`, at most `limits.max_concurrency` at
    /// once, within `limits`. `each` runs as each target finishes (its index and outcome);
    /// `done` runs once, with every target's outcome and plane, whatever failed.
    pub fn fanout<A: 'static>(
        targets: Vec<Address>,
        request: Outbound,
        transport: Transport,
        limits: FanoutLimits,
        each: impl FnMut(&mut A, usize, &Outcome) + 'static,
        done: impl FnOnce(&mut A, Vec<(Address, Outcome, Via)>) + 'static,
    ) -> FanoutHandle {
        fanout::start(
            std::rc::Rc::new(crate::ffi::Wasm),
            targets,
            request,
            transport,
            limits,
            each,
            done,
        )
    }

    impl ResponseWriter {
        /// The whole response; body chunks go out as the caller grants credit.
        pub fn respond(self, status: u16, headers: Headers, body: &[u8]) -> Result<(), Refused> {
            self.respond_with(&WASM, status, headers, body)
        }
    }

    /// Calls the internal HTTP server at one address, on one transport. There is no default
    /// transport: every client names one.
    #[derive(Clone, Debug)]
    pub struct Client {
        pub to: Address,
        pub transport: Transport,
    }

    impl Client {
        pub fn new(to: Address, transport: Transport) -> Client {
            Client { to, transport }
        }

        /// The response, whole, to `f` with the app.
        pub fn call<A: 'static>(
            &self,
            request: Outbound,
            f: impl FnOnce(&mut A, Result<Response, Refused>) + 'static,
        ) -> CallHandle {
            let key = client_whole::call(&WASM, self.to.clone(), self.transport, request, f);
            CallHandle { key }
        }

        /// The response as it streams, to `on` with the app.
        pub fn call_stream<A: 'static>(
            &self,
            request: Outbound,
            on: impl FnMut(&mut A, ClientEvent) + 'static,
        ) -> CallHandle {
            let key = client::call_stream(&WASM, self.to.clone(), self.transport, request, on);
            CallHandle { key }
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use wasm::*;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[cfg(test)]
#[path = "late_refusal_tests.rs"]
mod late_refusal_tests;
