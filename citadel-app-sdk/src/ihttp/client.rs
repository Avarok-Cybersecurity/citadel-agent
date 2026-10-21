//! The calling side: send a request under the server's credit, grant credit as response chunks
//! are read, and hand the response to the app as it streams.

use super::outbox::Outbox;
use super::{Outbound, Response};
use crate::abi::RawAbi;
use crate::pending::{spawn, start};
use citadel_app_sdk_types::calls::*;
use citadel_app_sdk_types::Refused;
use std::any::Any;
use std::cell::RefCell;
use std::collections::BTreeMap;

/// One step of a streamed response.
#[derive(Clone, Debug, PartialEq)]
pub enum ClientEvent {
    Head(ResponseHead),
    Body(Vec<u8>),
    End,
    Failed(Refused),
}

type Sink = Box<dyn FnMut(&mut dyn Any, ClientEvent)>;
type Key = (String, u64);

struct Stream {
    to: Address,
    sink: Option<Sink>,
    outbox: Outbox,
}

#[derive(Default)]
struct Clients {
    last_stream: u64,
    streams: BTreeMap<Key, Stream>,
}

thread_local! {
    static CLIENTS: RefCell<Clients> = RefCell::new(Clients::default());
}

fn target(to: &Address) -> (String, String) {
    match to {
        Address::User { app_id, .. } => (app_id.clone(), String::new()),
        Address::Scope { app_id, scope_key } => (app_id.clone(), scope_key.clone()),
    }
}

pub(crate) fn call_stream<A: 'static, R: RawAbi + 'static>(
    raw: &R,
    to: Address,
    request: Outbound,
    mut on: impl FnMut(&mut A, ClientEvent) + 'static,
) {
    let sink: Sink = Box::new(move |app, event| {
        on(
            app.downcast_mut::<A>()
                .expect("ihttp names the app's own type"),
            event,
        )
    });
    let (app_id, scope_key) = target(&to);
    let mut outbox = Outbox::new();
    let head = RequestHead {
        method: request.method,
        path: request.path,
        query: request.query,
        headers: request.headers,
        body_len: Some(request.body.len() as u64),
        app_id: app_id.clone(),
        scope_key,
    };
    outbox.message(FrameKind::RequestHead(head), &request.body);
    let k = CLIENTS.with(|c| {
        let mut c = c.borrow_mut();
        c.last_stream += 1;
        let k = (app_id, c.last_stream);
        c.streams.insert(
            k.clone(),
            Stream {
                to,
                sink: Some(sink),
                outbox,
            },
        );
        k
    });
    super::install::<R>();
    flush(raw, &k);
}

pub(crate) fn call<A: 'static, R: RawAbi + 'static>(
    raw: &R,
    to: Address,
    request: Outbound,
    f: impl FnOnce(&mut A, Result<Response, Refused>) + 'static,
) {
    let (mut f, mut head, mut body) = (Some(f), None, Vec::new());
    call_stream::<A, R>(raw, to, request, move |app, event| match event {
        ClientEvent::Head(h) => head = Some(h),
        ClientEvent::Body(b) => body.extend_from_slice(&b),
        ClientEvent::End => {
            let result = head.take().map(|head| Response {
                head,
                body: std::mem::take(&mut body),
            });
            if let Some(f) = f.take() {
                f(
                    app,
                    result.ok_or_else(|| Refused::Unavailable("response without a head".into())),
                );
            }
        }
        ClientEvent::Failed(e) => {
            if let Some(f) = f.take() {
                f(app, Err(e));
            }
        }
    });
}

/// End the stream with `event`: run later, since the app may be the caller.
fn finish(k: &Key, event: ClientEvent) {
    if let Some(mut stream) = CLIENTS.with(|c| c.borrow_mut().streams.remove(k)) {
        if let Some(mut sink) = stream.sink.take() {
            spawn(Box::new(move |app| sink(app, event)));
        }
    }
}

fn flush(raw: &impl RawAbi, k: &Key) {
    loop {
        let next = CLIENTS.with(|c| {
            c.borrow_mut()
                .streams
                .get_mut(k)
                .map(|s| (s.to.clone(), s.outbox.next()))
        });
        let Some((to, Some(kind))) = next else { return };
        let mut sent = start(
            raw,
            &IhttpCall {
                to,
                frame: IhttpFrame::new(k.1, kind),
            },
        );
        match sent.take_ready() {
            Some(Err(e)) => return finish(k, ClientEvent::Failed(e)),
            Some(Ok(())) => {}
            None => sent.watch("ihttp call"),
        }
    }
}

fn deliver(k: &Key, app: &mut dyn Any, event: ClientEvent, last: bool) {
    let sink = CLIENTS.with(|c| {
        c.borrow_mut()
            .streams
            .get_mut(k)
            .and_then(|s| s.sink.take())
    });
    let Some(mut sink) = sink else { return };
    sink(app, event);
    CLIENTS.with(|c| {
        let mut c = c.borrow_mut();
        if last {
            c.streams.remove(k);
        } else if let Some(s) = c.streams.get_mut(k) {
            s.sink = Some(sink);
        }
    });
}

/// A frame on a stream this instance opened; the frame back when it is not one.
pub(crate) fn on_frame(
    raw: &impl RawAbi,
    from: Caller,
    frame: IhttpFrame,
    app: &mut dyn Any,
) -> Option<(Caller, IhttpFrame)> {
    let k = (from.app_id.clone(), frame.stream);
    let Some(to) = CLIENTS.with(|c| c.borrow().streams.get(&k).map(|s| s.to.clone())) else {
        return Some((from, frame));
    };
    match frame.kind {
        FrameKind::ResponseHead(h) => deliver(&k, app, ClientEvent::Head(h), false),
        FrameKind::Body { bytes } => {
            deliver(&k, app, ClientEvent::Body(bytes.0), false);
            let credit = IhttpFrame::new(k.1, FrameKind::Credit { n: 1 });
            start(raw, &IhttpCall { to, frame: credit }).watch("ihttp credit");
        }
        FrameKind::End => deliver(&k, app, ClientEvent::End, true),
        FrameKind::Cancel { reason } => deliver(
            &k,
            app,
            ClientEvent::Failed(Refused::Unavailable(reason)),
            true,
        ),
        FrameKind::Credit { n } => {
            CLIENTS.with(|c| {
                c.borrow_mut()
                    .streams
                    .get_mut(&k)
                    .map(|s| s.outbox.grant(n))
            });
            flush(raw, &k);
        }
        kind @ FrameKind::RequestHead(_) => return Some((from, IhttpFrame::new(k.1, kind))),
    }
    None
}
