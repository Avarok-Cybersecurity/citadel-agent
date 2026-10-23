use super::*;
use crate::ihttp::tests::{caller, ok_host, sent, App};
use crate::pending::drain;
use citadel_app_sdk_types::calls::*;
use citadel_app_sdk_types::{Bytes, Refused};

#[derive(Default)]
struct Pool {
    each: Vec<(usize, Outcome)>,
    done: Vec<Vec<(Address, Outcome, Via)>>,
}

fn to(username: &str) -> Address {
    Address::User {
        app_id: "me".into(),
        username: username.into(),
    }
}

fn job() -> Outbound {
    Outbound {
        method: HttpMethod::Post,
        path: "/job".into(),
        query: String::new(),
        headers: vec![],
        body: b"chunk".to_vec(),
    }
}

fn limits(max_concurrency: u32, max_total_bytes: u64) -> FanoutLimits {
    FanoutLimits {
        max_concurrency,
        per_call_ms: 100,
        budget_ms: 1000,
        max_total_bytes,
    }
}

/// `(username, stream)` of every request the pool opened, in order.
fn opened(fake: &crate::abi::tests::Fake) -> Vec<(String, u64)> {
    sent::<IhttpCall>(fake)
        .into_iter()
        .filter(|c| matches!(c.frame.kind, FrameKind::RequestHead(_)))
        .map(|c| match c.to {
            Address::User { username, .. } => (username, c.frame.stream),
            Address::Scope { scope_key, .. } => (scope_key, c.frame.stream),
        })
        .collect()
}

fn start_pool(fake: &Rc<crate::abi::tests::Fake>, n: usize, l: FanoutLimits) -> FanoutHandle {
    let targets = (0..n).map(|i| to(&format!("w{i}"))).collect();
    start(
        fake.clone(),
        targets,
        job(),
        Transport::PreferP2pThenRelay,
        l,
        |p: &mut Pool, i, o: &Outcome| p.each.push((i, o.clone())),
        |p: &mut Pool, all| p.done.push(all),
    )
}

fn respond(fake: &crate::abi::tests::Fake, pool: &mut Pool, stream: u64, via: Via, body: &[u8]) {
    let mut from = caller("me");
    from.via = via;
    let head = ResponseHead {
        status: 200,
        headers: vec![],
        body_len: Some(body.len() as u64),
    };
    let mut route = |kind| {
        let out = crate::ihttp::route(fake, from.clone(), IhttpFrame::new(stream, kind), pool);
        drain(pool);
        out.is_none()
    };
    assert!(route(FrameKind::ResponseHead(head)));
    assert!(route(FrameKind::Body {
        bytes: Bytes(body.to_vec()),
    }));
    // A stream the pool stopped is closed: its End is no longer the pool's.
    let _ = route(FrameKind::End);
}

#[test]
fn a_fanout_runs_at_most_the_cap_and_done_has_every_outcome_with_its_plane() {
    let fake = Rc::new(ok_host());
    let mut pool = Pool::default();
    let _h = start_pool(&fake, 3, limits(2, 1 << 20));
    drain(&mut pool);
    let first = opened(&fake);
    assert_eq!(first.len(), 2, "the cap");
    assert!(sent::<IhttpCall>(&fake)
        .iter()
        .all(|c| c.transport == Transport::PreferP2pThenRelay));
    respond(&fake, &mut pool, first[0].1, Via::Relay, b"7");
    let all = opened(&fake);
    assert_eq!(all.len(), 3, "a settled call made room");
    let cancel = FrameKind::Cancel {
        reason: "refused".into(),
    };
    let from = caller("me");
    let out = crate::ihttp::route(&*fake, from, IhttpFrame::new(all[1].1, cancel), &mut pool);
    drain(&mut pool);
    assert!(out.is_none());
    respond(&fake, &mut pool, all[2].1, Via::P2p, b"9");
    assert_eq!(pool.each.len(), 3);
    let [done] = &pool.done[..] else {
        panic!("done once: {:?}", pool.done.len())
    };
    let vias: Vec<Via> = done.iter().map(|d| d.2).collect();
    assert_eq!(vias, [Via::Relay, Via::P2p, Via::P2p]);
    assert!(matches!(&done[0].1, Outcome::Ok(r) if r.body == b"7" && r.via == Via::Relay));
    assert_eq!(
        done[1].1,
        Outcome::Failed(Refused::Unavailable("refused".into()))
    );
    assert_eq!(done[0].0, to("w0"));
}

#[test]
fn time_and_bytes_stop_calls_with_their_limit_and_the_rest_are_skipped() {
    let fake = Rc::new(ok_host());
    let mut pool = Pool::default();
    let _h = start_pool(&fake, 4, limits(2, 4));
    drain(&mut pool);
    let first = opened(&fake);
    respond(&fake, &mut pool, first[0].1, Via::P2p, b"too long");
    let limit = Outcome::Failed(Refused::Limit(LIMIT_TOTAL_BYTES_FOR_TEST.into()));
    assert_eq!(pool.each[0], (0, limit));
    let cancels = sent::<IhttpCall>(&fake)
        .into_iter()
        .filter(|c| {
            c.frame.stream == first[0].1 && matches!(c.frame.kind, FrameKind::Cancel { .. })
        })
        .count();
    assert_eq!(cancels, 1, "the stopped stream is cancelled at the server");
    tick(100, &mut pool);
    drain(&mut pool);
    let per_call = Outcome::Failed(Refused::Limit(
        super::super::fanout_plan::LIMIT_PER_CALL_MS.into(),
    ));
    assert!(pool.each.contains(&(1, per_call.clone())));
    assert!(
        pool.each.contains(&(2, per_call)),
        "started when 0 settled: 100 ms too"
    );
    tick(1000, &mut pool);
    drain(&mut pool);
    let [done] = &pool.done[..] else {
        panic!("done once")
    };
    assert_eq!(
        done[3].1,
        Outcome::Skipped,
        "the budget ran out before it finished"
    );
}

const LIMIT_TOTAL_BYTES_FOR_TEST: &str = super::super::fanout_plan::LIMIT_TOTAL_BYTES;

#[test]
fn cancelling_skips_what_runs_and_waits_and_done_still_fires_once() {
    let fake = Rc::new(ok_host());
    let mut pool = Pool::default();
    let h = start_pool(&fake, 4, limits(1, 1 << 20));
    drain(&mut pool);
    cancel(h.id);
    drain(&mut pool);
    let [done] = &pool.done[..] else {
        panic!("done once")
    };
    assert!(done.iter().all(|d| d.1 == Outcome::Skipped));
    assert_eq!(opened(&fake).len(), 1, "nothing new starts once cancelled");
}

#[test]
fn an_empty_fanout_is_done_at_once() {
    let fake = Rc::new(ok_host());
    let mut pool = Pool::default();
    let _h = start_pool(&fake, 0, limits(1, 1));
    drain(&mut pool);
    assert_eq!(pool.done, [vec![]]);
}

#[test]
fn a_cancelled_call_sends_cancel_caller_on_its_transport_and_fails_once() {
    let fake = ok_host();
    let mut app = App::default();
    let to = Address::User {
        app_id: "me".into(),
        username: "bob".into(),
    };
    let out = Outbound {
        method: HttpMethod::Get,
        path: "/slow".into(),
        query: String::new(),
        headers: vec![],
        body: vec![],
    };
    let k =
        crate::ihttp::client_whole::call(&fake, to, Transport::Relay, out, |app: &mut App, r| {
            app.responses.push(r)
        });
    crate::ihttp::client::cancel(&fake, &k);
    drain(&mut app);
    let last = sent::<IhttpCall>(&fake).pop().unwrap();
    assert_eq!(last.transport, Transport::Relay);
    assert_eq!(
        last.frame.kind,
        FrameKind::Cancel {
            reason: "caller".into()
        }
    );
    assert_eq!(app.responses, [Err(Refused::Unavailable("caller".into()))]);
    crate::ihttp::client::cancel(&fake, &k);
    drain(&mut app);
    assert_eq!(
        app.responses.len(),
        1,
        "a closed stream is not cancelled twice"
    );
}
