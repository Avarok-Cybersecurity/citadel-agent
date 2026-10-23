use super::*;
use crate::abi::tests::Fake;
use crate::pending::drain;
use citadel_app_sdk_types::calls::{Caller, TopicOpened, Via};
use citadel_app_sdk_types::{decode, encode, HostOp, HostReply};

#[derive(Default)]
struct Pool {
    seen: Vec<TopicEvent>,
    topic: Option<Topic>,
}

fn sent<O: HostOp>(fake: &Fake) -> Vec<O> {
    fake.calls
        .borrow()
        .iter()
        .filter(|c| c.1 == O::OP && c.0 == O::GROUP)
        .map(|c| decode::<O>(&c.2).unwrap())
        .collect()
}

fn opened(members: &[&str]) -> Fake {
    Fake::answering(encode(&HostReply::Ok(TopicOpened {
        name: "pool".into(),
        members: members.iter().map(|m| m.to_string()).collect(),
    })))
}

fn message(body: &[u8]) -> TopicEvent {
    let from = Caller {
        username: "bob".into(),
        cid: 2,
        app_id: "net.avarok.compute-pool".into(),
        install_id: "i".into(),
        via: Via::Group,
    };
    TopicEvent::Message {
        from,
        body: Bytes(body.to_vec()),
    }
}

#[test]
fn an_open_topic_takes_its_events_and_tracks_its_members() {
    let fake = opened(&["alice"]);
    let mut pool = Pool::default();
    open_with(&fake, "pool", |p: &mut Pool, e| p.seen.push(e)).then(|p: &mut Pool, t| {
        p.topic = Some(t.unwrap());
    });
    drain(&mut pool);
    assert_eq!(sent::<TopicOpen>(&fake)[0].name, "pool");
    let topic = pool.topic.take().unwrap();
    assert_eq!(topic.members(), ["alice"]);
    assert!(route(&mut pool, "pool".into(), message(b"hi")).is_none());
    let members = TopicEvent::Members(vec!["alice".into(), "bob".into()]);
    assert!(route(&mut pool, "pool".into(), members.clone()).is_none());
    assert_eq!(pool.seen, [message(b"hi"), members]);
    assert_eq!(topic.members(), ["alice", "bob"]);
    let other = route(&mut pool, "other".into(), message(b"x"));
    assert_eq!(
        other,
        Some(("other".into(), message(b"x"))),
        "not opened: the app's"
    );
    topic.close_with(&fake);
    assert_eq!(sent::<TopicClose>(&fake)[0].name, "pool");
    assert!(
        route(&mut pool, "pool".into(), message(b"late")).is_some(),
        "closed"
    );
}

#[test]
fn a_publish_refused_for_its_rate_fails_at_once_and_one_accepted_is_ok() {
    let fake = opened(&[]);
    let mut pool = Pool::default();
    open_with(&fake, "pool", |_: &mut Pool, _| {}).then(|p: &mut Pool, t| {
        p.topic = Some(t.unwrap());
    });
    drain(&mut pool);
    let topic = pool.topic.take().unwrap();
    let rate = Refused::Limit(LIMIT_TOPIC_RATE.into());
    fake.queue
        .borrow_mut()
        .push_back(encode(&HostReply::<()>::Refused(rate.clone())));
    assert_eq!(topic.publish_with(&fake, b"a"), Err(rate));
    fake.queue
        .borrow_mut()
        .push_back(encode(&HostReply::Ok(())));
    assert_eq!(topic.publish_with(&fake, b"b"), Ok(()));
    assert_eq!(
        topic.publish_with(&fake, b"c"),
        Ok(()),
        "a pending reply is not a refusal"
    );
    let bodies: Vec<Vec<u8>> = sent::<TopicPublish>(&fake)
        .into_iter()
        .map(|p| p.body.0)
        .collect();
    assert_eq!(bodies, [b"a".to_vec(), b"b".to_vec(), b"c".to_vec()]);
}
