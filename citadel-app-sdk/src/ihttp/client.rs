//! The calling side: send a request under the server's credit, return credit (IhttpConsumed, as
//! a server does) as response chunks are read, and hand the response to the app as it streams.

use super::outbox::Outbox;
use super::Outbound;
use crate::abi::RawAbi;
use crate::pending::{spawn, start};
use citadel_app_sdk_types::calls::*;
use citadel_app_sdk_types::Refused;
use std::any::Any;
use std::cell::RefCell;
use std::collections::BTreeMap;

/// One step of a streamed response. `Head` says which plane answered.
#[derive(Clone, Debug, PartialEq)]
pub enum ClientEvent {
    Head(ResponseHead, Via),
    Body(Vec<u8>),
    End,
    Failed(Refused),
}

pub(crate) type Sink = Box<dyn FnMut(&mut dyn Any, ClientEvent)>;
pub(crate) type Key = (String, u64);

struct Stream {
    to: Address,
    transport: Transport,
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
    transport: Transport,
    request: Outbound,
    mut on: impl FnMut(&mut A, ClientEvent) + 'static,
) -> Key {
    let sink: Sink = Box::new(move |app, event| {
        on(
            app.downcast_mut::<A>()
                .expect("ihttp names the app's own type"),
            event,
        )
    });
    open(raw, to, transport, request, sink)
}

/// Open a stream whose events go to `sink`, with the app as `dyn Any`.
pub(crate) fn open<R: RawAbi + 'static>(
    raw: &R,
    to: Address,
    transport: Transport,
    request: Outbound,
    sink: Sink,
) -> Key {
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
                transport,
                sink: Some(sink),
                outbox,
            },
        );
        k
    });
    super::install::<R>();
    flush(raw, &k);
    k
}

/// The caller gives up: `Cancel` goes to the server and the stream ends with `Failed`.
pub(crate) fn cancel(raw: &impl RawAbi, k: &Key) {
    let open = CLIENTS.with(|c| {
        c.borrow()
            .streams
            .get(k)
            .map(|s| (s.to.clone(), s.transport))
    });
    let Some((to, transport)) = open else { return };
    let frame = IhttpFrame::new(
        k.1,
        FrameKind::Cancel {
            reason: CANCEL_CALLER.into(),
        },
    );
    start(
        raw,
        &IhttpCall {
            to,
            frame,
            transport,
        },
    )
    .watch("ihttp cancel");
    finish(
        k,
        ClientEvent::Failed(Refused::Unavailable(CANCEL_CALLER.into())),
    );
}

/// The reason a caller's own `Cancel` names (`CancelReason::Caller` on the wire).
pub(crate) const CANCEL_CALLER: &str = "caller";

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
                .map(|s| (s.to.clone(), s.transport, s.outbox.next()))
        });
        let Some((to, transport, Some(kind))) = next else {
            return;
        };
        let frame = IhttpFrame::new(k.1, kind);
        let mut sent = start(
            raw,
            &IhttpCall {
                to,
                frame,
                transport,
            },
        );
        match sent.take_ready() {
            Some(Err(e)) => return finish(k, ClientEvent::Failed(e)),
            Some(Ok(())) => {}
            // Answered later (the host awaits the kernel or the agent): a refusal still ends the call.
            None => {
                let k = k.clone();
                sent.watch_then("ihttp call", move |e| finish(&k, ClientEvent::Failed(e)));
            }
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
    if !CLIENTS.with(|c| c.borrow().streams.contains_key(&k)) {
        return Some((from, frame));
    }
    match frame.kind {
        FrameKind::ResponseHead(h) => deliver(&k, app, ClientEvent::Head(h, from.via), false),
        FrameKind::Body { bytes } => {
            deliver(&k, app, ClientEvent::Body(bytes.0), false);
            let consumed = IhttpConsumed {
                peer: from,
                stream: k.1,
            };
            start(raw, &consumed).watch("ihttp consumed");
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
