use super::*;
use crate::abi::tests::Fake;
use crate::pending::drain;
use crate::{App, AppContext, AppEvent, CloseContext, CloseReason, InitFailure, SuspendReason};
use citadel_app_sdk_types::{decode, encode, Group, HostOp, HostReply};

#[derive(Default)]
struct Client {
    state: Option<Result<SessionState, Refused>>,
    seen: Vec<AppEvent>,
}

impl App for Client {
    fn init(_: AppContext) -> Result<Self, InitFailure> {
        Ok(Client::default())
    }
    fn event(&mut self, event: AppEvent) {
        self.seen.push(event);
    }
    fn suspend(&mut self, _: SuspendReason) {}
    fn resume(&mut self) {}
    fn close(&mut self, _: &CloseContext, _: CloseReason) {}
}

fn sent<O: HostOp>(fake: &Fake) -> Vec<O> {
    let calls = fake.calls.borrow();
    let ours = calls.iter().filter(|c| c.0 == O::GROUP && c.1 == O::OP);
    ours.map(|c| decode::<O>(&c.2).unwrap()).collect()
}

#[test]
fn open_calls_the_session_group_and_answers_the_hosts_state() {
    let state = SessionState {
        session: 4,
        cid: 77,
        state: SessionStatus::Connecting,
    };
    let fake = Fake::answering(encode(&HostReply::Ok(state.clone())));
    let mut client = Client::default();
    open_with(&fake, "synchronized.work").then(|c: &mut Client, r| c.state = Some(r));
    drain(&mut client);
    assert_eq!(fake.calls.borrow()[0].0, Group::Session);
    assert_eq!(sent::<SessionOpen>(&fake)[0].server, "synchronized.work");
    assert_eq!(client.state, Some(Ok(state)));
}

#[test]
fn a_send_refused_for_its_size_fails_at_once_and_a_pending_one_is_ok() {
    let bytes = Refused::Limit(LIMIT_SESSION_BYTES.into());
    let fake = Fake::answering(encode(&HostReply::<()>::Refused(bytes.clone())));
    assert_eq!(send_with(&fake, 4, b"big"), Err(bytes));
    fake.queue
        .borrow_mut()
        .push_back(encode(&HostReply::Ok(())));
    assert_eq!(send_with(&fake, 4, b"ok"), Ok(()));
    assert_eq!(
        send_with(&fake, 4, b"later"),
        Ok(()),
        "pending is not refused"
    );
    let bodies: Vec<(u32, Vec<u8>)> = sent::<SessionSend>(&fake)
        .into_iter()
        .map(|s| (s.session, s.body.0))
        .collect();
    let want = [
        (4, b"big".to_vec()),
        (4, b"ok".to_vec()),
        (4, b"later".to_vec()),
    ];
    assert_eq!(bodies, want);
    close_with(&fake, 4);
    assert_eq!(sent::<SessionClose>(&fake), [SessionClose { session: 4 }]);
}

#[test]
fn session_and_activate_events_reach_the_apps_event() {
    let mut client = Client::default();
    let message = AppEvent::Session {
        session: 4,
        event: SessionEvent::Message {
            body: Bytes(b"hi".to_vec()),
        },
    };
    let activate = AppEvent::Activate { id: "n1".into() };
    crate::route::deliver(&Fake::default(), &mut client, message.clone());
    crate::route::deliver(&Fake::default(), &mut client, activate.clone());
    assert_eq!(client.seen, [message, activate]);
}
