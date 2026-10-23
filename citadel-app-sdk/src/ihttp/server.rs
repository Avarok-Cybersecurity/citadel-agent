//! The serving side: assemble requests from frames, grant credit as chunks are read, send the
//! response under the caller's credit.

use super::outbox::Outbox;
use super::Request;
use crate::abi::RawAbi;
use crate::pending::{start, Pending};
use citadel_app_sdk_types::calls::*;
use citadel_app_sdk_types::Refused;
use std::any::Any;
use std::cell::RefCell;
use std::collections::BTreeMap;

type Handler = Box<dyn FnMut(&mut dyn Any, Request, ResponseWriter)>;
type Key = (u64, String, u64);

struct Incoming {
    caller: Caller,
    head: RequestHead,
    body: Vec<u8>,
}

struct Outgoing {
    peer: Caller,
    outbox: Outbox,
}

#[derive(Default)]
struct Server {
    handler: Option<Handler>,
    incoming: BTreeMap<Key, Incoming>,
    outgoing: BTreeMap<Key, Outgoing>,
}

thread_local! {
    static SERVER: RefCell<Server> = RefCell::new(Server::default());
}

fn key(c: &Caller, stream: u64) -> Key {
    (c.cid, c.install_id.clone(), stream)
}

pub(crate) fn owns(from: &Caller, stream: u64) -> bool {
    let k = key(from, stream);
    SERVER.with(|s| {
        let s = s.borrow();
        s.incoming.contains_key(&k) || s.outgoing.contains_key(&k)
    })
}

pub(crate) fn serve<A: 'static, R: RawAbi + 'static>(
    raw: &R,
    routes: Vec<Route>,
    on: ServeOn,
    mut handler: impl FnMut(&mut A, Request, ResponseWriter) + 'static,
) -> Pending<()> {
    let handler: Handler = Box::new(move |app, request, writer| {
        handler(
            app.downcast_mut::<A>()
                .expect("ihttp::serve names the app's own type"),
            request,
            writer,
        )
    });
    SERVER.with(|s| s.borrow_mut().handler = Some(handler));
    super::install::<R>();
    start(raw, &IhttpServe { routes, on })
}

fn complete(from: Caller, stream: u64, app: &mut dyn Any) {
    let k = key(&from, stream);
    let Some(inc) = SERVER.with(|s| s.borrow_mut().incoming.remove(&k)) else {
        return;
    };
    let outgoing = Outgoing {
        peer: from,
        outbox: Outbox::new(),
    };
    let handler = SERVER.with(|s| {
        let mut s = s.borrow_mut();
        s.outgoing.insert(k, outgoing);
        s.handler.take()
    });
    let Some(mut handler) = handler else { return };
    let h = inc.head;
    let request = Request {
        caller: inc.caller.clone(),
        stream,
        method: h.method,
        path: h.path,
        query: h.query,
        headers: h.headers,
        body: inc.body,
    };
    handler(
        app,
        request,
        ResponseWriter {
            peer: inc.caller,
            stream,
        },
    );
    SERVER.with(|s| {
        s.borrow_mut().handler.get_or_insert(handler);
    });
}

/// A server-side frame; the frame back when it is not one.
pub(crate) fn on_frame(
    raw: &impl RawAbi,
    from: Caller,
    frame: IhttpFrame,
    app: &mut dyn Any,
) -> Option<(Caller, IhttpFrame)> {
    let (stream, k) = (frame.stream, key(&from, frame.stream));
    match frame.kind {
        FrameKind::RequestHead(head) => {
            if SERVER.with(|s| s.borrow().handler.is_none()) {
                return Some((from, IhttpFrame::new(stream, FrameKind::RequestHead(head))));
            }
            let inc = Incoming {
                caller: from,
                head,
                body: Vec::new(),
            };
            SERVER.with(|s| s.borrow_mut().incoming.insert(k, inc));
        }
        FrameKind::Body { bytes } => {
            let read = SERVER.with(|s| {
                s.borrow_mut()
                    .incoming
                    .get_mut(&k)
                    .map(|i| i.body.extend_from_slice(&bytes.0))
            });
            if read.is_some() {
                start(raw, &IhttpConsumed { peer: from, stream }).watch("ihttp consumed");
            }
        }
        FrameKind::End => complete(from, stream, app),
        FrameKind::Cancel { .. } => SERVER.with(|s| {
            let mut s = s.borrow_mut();
            s.incoming.remove(&k);
            s.outgoing.remove(&k);
        }),
        FrameKind::Credit { n } => {
            SERVER.with(|s| {
                s.borrow_mut()
                    .outgoing
                    .get_mut(&k)
                    .map(|o| o.outbox.grant(n))
            });
            if let Err(e) = flush(raw, &k) {
                crate::pending::note(format!("ihttp response: {e:?}"));
            }
        }
        kind @ FrameKind::ResponseHead(_) => return Some((from, IhttpFrame::new(stream, kind))),
    }
    None
}

/// Send what the credit allows; the stream is closed after `End` or on a refusal.
fn flush(raw: &impl RawAbi, k: &Key) -> Result<(), Refused> {
    loop {
        let next = SERVER.with(|s| {
            s.borrow_mut()
                .outgoing
                .get_mut(k)
                .map(|o| (o.peer.clone(), o.outbox.next()))
        });
        let Some((peer, Some(kind))) = next else {
            return Ok(());
        };
        let last = matches!(kind, FrameKind::End | FrameKind::Cancel { .. });
        let mut sent = start(
            raw,
            &IhttpRespond {
                peer,
                frame: IhttpFrame::new(k.2, kind),
            },
        );
        match sent.take_ready() {
            Some(Err(e)) => {
                SERVER.with(|s| s.borrow_mut().outgoing.remove(k));
                return Err(e);
            }
            Some(Ok(())) => {}
            // Refused later (the host awaits the agent or the server): the stream is closed then.
            None => {
                let k = k.clone();
                sent.watch_then("ihttp respond", move |_| {
                    SERVER.with(|s| s.borrow_mut().outgoing.remove(&k));
                });
            }
        }
        if last {
            SERVER.with(|s| s.borrow_mut().outgoing.remove(k));
            return Ok(());
        }
    }
}

/// The response to one served request.
#[derive(Debug)]
pub struct ResponseWriter {
    peer: Caller,
    stream: u64,
}

impl ResponseWriter {
    pub(crate) fn respond_with(
        self,
        raw: &impl RawAbi,
        status: u16,
        headers: Headers,
        body: &[u8],
    ) -> Result<(), Refused> {
        let k = key(&self.peer, self.stream);
        let head = FrameKind::ResponseHead(ResponseHead {
            status,
            headers,
            body_len: Some(body.len() as u64),
        });
        let queued = SERVER.with(|s| {
            s.borrow_mut()
                .outgoing
                .get_mut(&k)
                .map(|o| o.outbox.message(head, body))
        });
        if queued.is_none() {
            return Err(Refused::Unavailable("the stream is closed".into()));
        }
        flush(raw, &k)
    }
}
