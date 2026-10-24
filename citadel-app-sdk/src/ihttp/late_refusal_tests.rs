//! A stream's frames go out without waiting for their answers, so a refusal can arrive after the
//! SDK moved on. It must still end the stream: a call, once, rather than leave the app waiting
//! for a response that cannot come (the fan-out's `fanout_per_call_ms`, ten seconds later); a
//! served response, so credit arriving later sends nothing more on it.

use super::tests::{caller, feed, sent, App};
use crate::abi::tests::Fake;
use crate::pending::{drain, reply_ready, take_notes};
use citadel_app_sdk_types::calls::*;
use citadel_app_sdk_types::{encode, HostOp, HostReply, Refused};

fn call(fake: &Fake) {
    let to = Address::User {
        app_id: "net.avarok.compute-pool".into(),
        username: "bob".into(),
    };
    let out = super::Outbound {
        method: HttpMethod::Post,
        path: "/job/chunks".into(),
        query: String::new(),
        headers: vec![],
        body: b"work".to_vec(),
    };
    super::client_whole::call(fake, to, Transport::P2p, out, |app: &mut App, r| {
        app.responses.push(r)
    });
}

/// Answer the pending reply `handle` with `reply` and run what it wakes.
fn answer(fake: &Fake, app: &mut App, handle: i32, reply: HostReply<()>) {
    fake.resolve(handle, encode(&reply));
    assert!(reply_ready(fake, handle), "the SDK owns the handle");
    drain(app);
}

#[test]
fn a_head_refused_after_the_call_returned_ends_the_call_with_that_refusal() {
    // Every reply pending: the head, the body and End (handles 1, 2, 3).
    let fake = Fake::default();
    let mut app = App::default();
    call(&fake);
    drain(&mut app);
    assert!(app.responses.is_empty(), "nothing answered yet");
    let refused = Refused::Unavailable("refused".into());
    answer(&fake, &mut app, 1, HostReply::Refused(refused.clone()));
    assert_eq!(app.responses, [Err(refused)]);
    // The frames behind it are refused too; the call already ended, so they change nothing.
    answer(
        &fake,
        &mut app,
        2,
        HostReply::Refused(Refused::Policy("unknown_stream".into())),
    );
    answer(
        &fake,
        &mut app,
        3,
        HostReply::Refused(Refused::Policy("unknown_stream".into())),
    );
    assert_eq!(app.responses.len(), 1, "ended once");
    assert_eq!(
        take_notes().len(),
        3,
        "each refusal is still noted for the log"
    );
}

#[test]
fn control_frames_answered_ok_later_leave_the_call_open_for_its_response() {
    let fake = Fake::default();
    let mut app = App::default();
    call(&fake);
    for handle in 1..=3 {
        answer(&fake, &mut app, handle, HostReply::Ok(()));
    }
    assert!(app.responses.is_empty());
    assert!(take_notes().is_empty());
}

#[test]
fn a_response_frame_refused_later_closes_the_served_stream() {
    let fake = std::rc::Rc::new(Fake::default());
    let respond_host = fake.clone();
    let mut app = App::default();
    let routes = vec![Route {
        method: HttpMethod::Post,
        path: "/x".into(),
    }];
    let on = ServeOn {
        relay: false,
        p2p: true,
    };
    let _serving = super::server::serve(&*fake, routes, on, move |_: &mut App, _, w| {
        // Larger than the initial credit, so the rest waits for Credit.
        w.respond_with(&*respond_host, 200, vec![], &vec![7u8; 6 * 65_536])
            .unwrap();
    });
    let from = caller("net.avarok.compute-pool");
    let head = FrameKind::RequestHead(RequestHead {
        method: HttpMethod::Post,
        path: "/x".into(),
        query: String::new(),
        headers: vec![],
        body_len: Some(0),
        app_id: "me".into(),
        scope_key: "s".into(),
    });
    for kind in [head, FrameKind::End] {
        assert!(feed(&fake, &mut app, &from, 5, kind).is_none());
    }
    let calls = fake.calls.borrow().clone();
    let first_respond = calls
        .iter()
        .position(|c| c.1 == IhttpRespond::OP)
        .expect("responded");
    let handle = first_respond as i32 + 1;
    answer(
        &fake,
        &mut app,
        handle,
        HostReply::Refused(Refused::Policy("unknown_stream".into())),
    );
    let before = sent::<IhttpRespond>(&fake).len();
    let more = feed(&fake, &mut app, &from, 5, FrameKind::Credit { n: 2 });
    assert!(
        more.is_some(),
        "the stream is closed: the credit is not its"
    );
    assert_eq!(
        sent::<IhttpRespond>(&fake).len(),
        before,
        "nothing more was sent"
    );
    take_notes();
}
