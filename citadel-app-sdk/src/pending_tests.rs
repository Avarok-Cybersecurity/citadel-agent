use super::*;
use crate::abi::tests::Fake;
use citadel_app_sdk_types::calls::{LogLevel, LogWrite, NodeGet, NodeInfo};
use citadel_app_sdk_types::HostReply;

#[derive(Default)]
struct App {
    got: Vec<Result<String, Refused>>,
}

fn node(name: &str) -> Vec<u8> {
    encode(&HostReply::Ok(NodeInfo {
        node_id: "n".into(),
        name: name.into(),
        parent_id: None,
        has_children: false,
    }))
}

fn get() -> NodeGet {
    NodeGet {
        node_id: "n".into(),
    }
}

#[test]
fn a_continuation_runs_after_the_reply_arrives_and_only_on_drain() {
    let fake = Fake::default();
    let p = start(&fake, &get()).map(|n| n.name);
    let h = p.handle().unwrap();
    p.then(|app: &mut App, r| app.got.push(r));
    let mut app = App::default();
    drain(&mut app);
    assert!(app.got.is_empty(), "nothing ran before the reply");
    assert!(
        !reply_ready(&fake, h + 100),
        "an unknown handle is the app's"
    );
    fake.resolve(h, node("late"));
    assert!(reply_ready(&fake, h));
    assert!(app.got.is_empty(), "queued, not run inside reply_ready");
    drain(&mut app);
    assert_eq!(app.got, [Ok("late".to_string())]);
}

#[test]
fn an_immediate_reply_also_waits_for_drain() {
    let fake = Fake::answering(node("now"));
    start(&fake, &get())
        .map(|n| n.name)
        .then(|app: &mut App, r| app.got.push(r));
    let mut app = App::default();
    drain(&mut app);
    assert_eq!(app.got, [Ok("now".to_string())]);
}

#[test]
fn take_ready_answers_refusals_at_once_and_none_while_pending() {
    let stub = b"\xa1\x67refused\x69ungranted".to_vec();
    let mut refused = start(&Fake::answering(stub), &get());
    assert_eq!(refused.take_ready(), Some(Err(Refused::Ungranted)));
    let fake = Fake::default();
    let mut waiting = start(&fake, &get());
    assert_eq!(waiting.take_ready(), None);
    assert_eq!(waiting.poll_with(&fake), None);
    fake.resolve(waiting.handle().unwrap(), node("x"));
    assert_eq!(waiting.poll_with(&fake).unwrap().unwrap().name, "x");
}

#[test]
fn a_dropped_pending_is_read_and_discarded_when_it_arrives() {
    let fake = Fake::default();
    let h = start(&fake, &get()).handle().unwrap();
    fake.resolve(h, node("x"));
    assert!(reply_ready(&fake, h), "the SDK owns a dropped handle");
    assert!(fake.replies.borrow().is_empty(), "and released it");
    let mut app = App::default();
    drain(&mut app);
    assert!(app.got.is_empty());
}

#[test]
fn an_immediate_op_refuses_a_pending_answer_instead_of_blocking() {
    let log = LogWrite {
        level: LogLevel::Info,
        message: "m".into(),
    };
    assert_eq!(
        now(&Fake::answering(encode(&HostReply::Ok(()))), &log),
        Ok(())
    );
    let fake = Fake::default();
    assert_eq!(
        now(&fake, &log),
        Err(Refused::Unavailable(
            "an immediate op answered pending".into()
        ))
    );
}
