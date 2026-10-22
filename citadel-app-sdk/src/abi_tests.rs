use super::*;
use citadel_app_sdk_types::calls::{NodeGet, NodeInfo};
use citadel_app_sdk_types::{encode, HostOp};
use std::cell::RefCell;

/// A host: each invoke issues the next handle, ready with the next queued reply or pending when
/// the queue is empty. It records every call.
#[derive(Default)]
pub(crate) struct Fake {
    pub queue: RefCell<std::collections::VecDeque<Vec<u8>>>,
    pub replies: RefCell<std::collections::BTreeMap<i32, Option<Vec<u8>>>>,
    pub calls: RefCell<Vec<(Group, i32, Vec<u8>)>>,
    next: RefCell<i32>,
    pub invoke_code: Option<i32>,
    pub short_read: bool,
}

impl Fake {
    pub fn answering(reply: Vec<u8>) -> Fake {
        let f = Fake::default();
        f.queue.borrow_mut().push_back(reply);
        f
    }
    pub fn resolve(&self, handle: i32, reply: Vec<u8>) {
        self.replies.borrow_mut().insert(handle, Some(reply));
    }
}

impl RawAbi for Fake {
    fn invoke(&self, group: Group, op: i32, payload: &[u8]) -> i32 {
        self.calls.borrow_mut().push((group, op, payload.to_vec()));
        if let Some(code) = self.invoke_code {
            return code;
        }
        let handle = {
            let mut next = self.next.borrow_mut();
            *next += 1;
            *next
        };
        let reply = self.queue.borrow_mut().pop_front();
        self.replies.borrow_mut().insert(handle, reply);
        handle
    }
    fn reply_len(&self, handle: i32) -> i32 {
        match self.replies.borrow().get(&handle) {
            None => -2,
            Some(None) => -3,
            Some(Some(r)) => r.len() as i32,
        }
    }
    fn reply_read(&self, handle: i32, buf: &mut [u8]) -> i32 {
        let mut replies = self.replies.borrow_mut();
        match replies.get(&handle) {
            None => return -2,
            Some(None) => return -3,
            Some(Some(_)) => {}
        }
        let Some(Some(r)) = replies.remove(&handle) else {
            unreachable!()
        };
        if self.short_read {
            return r.len() as i32 - 1;
        }
        buf.copy_from_slice(&r);
        r.len() as i32
    }
}

fn node() -> NodeInfo {
    NodeInfo {
        node_id: "n".into(),
        name: "Name".into(),
        parent_id: None,
        has_children: false,
    }
}

#[test]
fn an_immediate_reply_is_read_at_once_and_its_handle_released() {
    let fake = Fake::answering(encode(&HostReply::Ok(node())));
    let Sent::Done(Ok(bytes)) = send(
        &fake,
        Group::Nodes,
        NodeGet::OP,
        &encode(&NodeGet {
            node_id: "n".into(),
        }),
    ) else {
        panic!()
    };
    assert_eq!(decode_reply::<NodeInfo>(&bytes), Ok(node()));
    assert!(
        fake.replies.borrow().is_empty(),
        "one-shot read released the handle"
    );
}

#[test]
fn a_pending_reply_is_pending_until_resolved_then_reads() {
    let fake = Fake::default();
    let Sent::Pending(h) = send(&fake, Group::Nodes, 1, &[]) else {
        panic!()
    };
    assert_eq!(read(&fake, h), Sent::Pending(h));
    fake.resolve(h, vec![1, 2]);
    assert_eq!(read(&fake, h), Sent::Done(Ok(vec![1, 2])));
}

#[test]
fn the_ungranted_stub_decodes_as_ungranted_for_any_reply_type() {
    let stub = b"\xa1\x67refused\x69ungranted";
    assert_eq!(decode_reply::<NodeInfo>(stub), Err(Refused::Ungranted));
    assert_eq!(decode_reply::<()>(stub), Err(Refused::Ungranted));
    assert_eq!(
        decode_reply::<()>(&[0xff]),
        Err(Refused::Unavailable("malformed reply".into()))
    );
}

#[test]
fn negative_codes_become_refusals_not_panics() {
    for (code, want) in [
        (-1, Refused::Unavailable("BAD_REQUEST".into())),
        (-2, Refused::Unavailable("UNKNOWN_HANDLE".into())),
        (-4, Refused::Limit("reply".into())),
        (-5, Refused::Limit("handles".into())),
        (-99, Refused::Unavailable("abi error -99".into())),
    ] {
        let fake = Fake {
            invoke_code: Some(code),
            ..Fake::default()
        };
        assert_eq!(send(&fake, Group::Log, 1, &[]), Sent::Done(Err(want)));
    }
    assert_eq!(
        read(&Fake::default(), 77),
        Sent::Done(Err(Refused::Unavailable("UNKNOWN_HANDLE".into())))
    );
}

#[test]
fn a_short_read_is_refused_and_the_honest_control_delivers() {
    let short = Fake {
        short_read: true,
        ..Fake::answering(vec![1, 2, 3])
    };
    assert_eq!(
        send(&short, Group::Log, 1, &[]),
        Sent::Done(Err(Refused::Unavailable("short reply".into())))
    );
    assert_eq!(
        send(&Fake::answering(vec![1, 2, 3]), Group::Log, 1, &[]),
        Sent::Done(Ok(vec![1, 2, 3]))
    );
}
