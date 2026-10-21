use super::*;
use crate::abi::tests::Fake;
use crate::pending::drain;
use citadel_app_sdk_types::calls::*;
use citadel_app_sdk_types::{decode, encode, Bytes, HostOp, HostReply, Refused};

#[derive(Default)]
struct App {
    served: Vec<Request>,
    responses: Vec<Result<Response, Refused>>,
}

fn ok_host() -> Fake {
    let f = Fake::default();
    for _ in 0..64 {
        f.queue.borrow_mut().push_back(encode(&HostReply::Ok(())));
    }
    f
}

fn caller(app_id: &str) -> Caller {
    Caller {
        username: "bob".into(),
        cid: 9,
        app_id: app_id.into(),
        install_id: "i9".into(),
        via: Via::P2p,
    }
}

fn head(len: u64) -> FrameKind {
    let h = RequestHead {
        method: HttpMethod::Post,
        path: "/x".into(),
        query: String::new(),
        headers: vec![],
        body_len: Some(len),
        app_id: "me".into(),
        scope_key: "s".into(),
    };
    FrameKind::RequestHead(h)
}

fn body(b: &[u8]) -> FrameKind {
    FrameKind::Body {
        bytes: Bytes(b.to_vec()),
    }
}

/// The frames the app sent with `op`, decoded.
fn sent<O: HostOp>(fake: &Fake) -> Vec<O> {
    fake.calls
        .borrow()
        .iter()
        .filter(|c| c.1 == O::OP && c.0 == O::GROUP)
        .map(|c| decode::<O>(&c.2).unwrap())
        .collect()
}

fn feed(
    fake: &Fake,
    app: &mut App,
    from: &Caller,
    stream: u64,
    kind: FrameKind,
) -> Option<(Caller, IhttpFrame)> {
    let out = route(fake, from.clone(), IhttpFrame::new(stream, kind), app);
    drain(app);
    out
}

#[test]
fn without_a_handler_a_request_is_the_apps_own() {
    let mut app = App::default();
    assert!(feed(&ok_host(), &mut app, &caller("other"), 1, head(0)).is_some());
}

#[test]
fn a_served_request_is_assembled_each_chunk_credited_and_the_response_waits_for_credit() {
    let fake = std::rc::Rc::new(ok_host());
    let respond_host = fake.clone();
    let mut app = App::default();
    let routes = vec![Route {
        method: HttpMethod::Post,
        path: "/x".into(),
    }];
    let mut advertised = super::server::serve(
        &*fake,
        routes,
        move |app: &mut App, req: Request, w: ResponseWriter| {
            app.served.push(req);
            w.respond_with(&*respond_host, 200, vec![], &vec![7u8; 5 * 65_536 + 1])
                .unwrap();
        },
    );
    assert_eq!(advertised.take_ready(), Some(Ok(())));
    assert_eq!(sent::<IhttpServe>(&fake)[0].routes[0].path, "/x");
    let from = caller("other");
    for kind in [head(4), body(b"ab"), body(b"cd"), FrameKind::End] {
        assert!(feed(&fake, &mut app, &from, 3, kind).is_none());
    }
    assert_eq!(app.served.len(), 1);
    assert_eq!(app.served[0].body, b"abcd");
    let consumed = sent::<IhttpConsumed>(&fake);
    assert_eq!(consumed.len(), 2, "one credit per chunk read");
    assert!(consumed.iter().all(|c| c.stream == 3 && c.peer == from));

    let kinds = |f: &Fake| {
        sent::<IhttpRespond>(f)
            .into_iter()
            .map(|r| r.frame.kind)
            .collect::<Vec<_>>()
    };
    let k = kinds(&fake);
    assert!(matches!(
        k[0],
        FrameKind::ResponseHead(ResponseHead {
            status: 200,
            body_len: Some(327_681),
            ..
        })
    ));
    assert_eq!(k.len(), 5, "the head and four chunks: the initial credit");
    feed(&fake, &mut app, &from, 3, FrameKind::Credit { n: 2 });
    let k = kinds(&fake);
    assert_eq!(k.len(), 8, "two more chunks, the last one short, then End");
    assert!(matches!(&k[6], FrameKind::Body { bytes } if bytes.0.len() == 1));
    assert_eq!(k[7], FrameKind::End);
    assert!(
        feed(&fake, &mut app, &from, 3, FrameKind::Credit { n: 1 }).is_some(),
        "the stream is closed"
    );
}

#[test]
fn a_client_call_sends_head_body_end_and_assembles_the_response_crediting_each_chunk() {
    let fake = ok_host();
    let mut app = App::default();
    let to = Address::User {
        app_id: "com.acme.srv".into(),
        username: "carol".into(),
    };
    let out = Outbound {
        method: HttpMethod::Get,
        path: "/a".into(),
        query: "q=1".into(),
        headers: vec![("x".into(), "y".into())],
        body: b"hi".to_vec(),
    };
    super::client::call(&fake, to, out, |app: &mut App, r| app.responses.push(r));
    let frames: Vec<IhttpFrame> = sent::<IhttpCall>(&fake)
        .into_iter()
        .map(|c| c.frame)
        .collect();
    assert_eq!(frames.len(), 3);
    let FrameKind::RequestHead(h) = &frames[0].kind else {
        panic!("{frames:?}")
    };
    assert_eq!(
        (h.app_id.as_str(), h.body_len, h.query.as_str()),
        ("com.acme.srv", Some(2), "q=1")
    );
    let stream = frames[0].stream;
    let server = caller("com.acme.srv");
    let rh = ResponseHead {
        status: 201,
        headers: vec![],
        body_len: Some(3),
    };
    for kind in [
        FrameKind::ResponseHead(rh.clone()),
        body(b"o"),
        body(b"k!"),
        FrameKind::End,
    ] {
        assert!(feed(&fake, &mut app, &server, stream, kind).is_none());
    }
    assert_eq!(
        app.responses,
        [Ok(Response {
            head: rh,
            body: b"ok!".to_vec()
        })]
    );
    // Credit goes back the way a server returns it, never as an app-made Credit frame: the
    // host mints Credit, and a client holding only ihttp.call may send IhttpConsumed.
    let consumed = sent::<IhttpConsumed>(&fake);
    assert_eq!(consumed.len(), 2);
    assert!(consumed
        .iter()
        .all(|c| c.peer == server && c.stream == stream));
    let forged = sent::<IhttpCall>(&fake)
        .into_iter()
        .filter(|c| matches!(c.frame.kind, FrameKind::Credit { .. }))
        .count();
    assert_eq!(forged, 0);
    assert!(
        feed(&fake, &mut app, &server, stream, FrameKind::End).is_some(),
        "the stream is closed"
    );
}

#[test]
fn a_refused_call_fails_the_client_once_and_never_inside_the_callers_borrow() {
    let fake = Fake::answering(encode(&HostReply::<()>::Refused(Refused::Ungranted)));
    let mut app = App::default();
    let to = Address::Scope {
        app_id: "a.b".into(),
        scope_key: "k".into(),
    };
    let out = Outbound {
        method: HttpMethod::Get,
        path: "/".into(),
        query: String::new(),
        headers: vec![],
        body: vec![],
    };
    super::client::call(&fake, to, out, |app: &mut App, r| app.responses.push(r));
    assert!(app.responses.is_empty(), "not run inside call");
    drain(&mut app);
    assert_eq!(app.responses, [Err(Refused::Ungranted)]);
}
