//! The bytes on the wire, asserted literally: the host is written in another language from these.

use citadel_app_sdk_types::calls::*;
use citadel_app_sdk_types::ops::{CallDecodeError, NodesOps, StorageOps};
use citadel_app_sdk_types::{
    decode, encode, Bytes, Capability, Group, HostCall, HostOp, HostReply, Refused,
};

/// CBOR `{"refused": "ungranted"}`: a 1-entry map, text(7) "refused", text(9) "ungranted".
const UNGRANTED: &[u8] = b"\xa1\x67refused\x69ungranted";

#[test]
fn the_ungranted_stub_is_exactly_refused_ungranted() {
    assert_eq!(
        encode(&HostReply::<()>::Refused(Refused::Ungranted)),
        UNGRANTED
    );
    assert_eq!(
        encode(&HostReply::<NodeList>::Refused(Refused::Ungranted)),
        UNGRANTED
    );
    let any: HostReply<HttpResponse> = decode(UNGRANTED).unwrap();
    assert_eq!(any.into_result(), Err(Refused::Ungranted));
}

#[test]
fn refusals_and_ok_have_their_documented_shapes() {
    assert_eq!(
        encode(&Refused::Limit("reply".into())),
        b"\xa1\x65limit\x65reply"
    );
    assert_eq!(encode(&Refused::Remote(404)), b"\xa1\x66remote\x19\x01\x94");
    assert_eq!(encode(&HostReply::Ok(())), b"\xa1\x62ok\xf6");
    let ok: HostReply<Text> = decode(&encode(&HostReply::Ok(Text { text: "hi".into() }))).unwrap();
    assert_eq!(ok.into_result().unwrap().text, "hi");
}

#[test]
fn bytes_travel_as_one_byte_string() {
    assert_eq!(encode(&Bytes(vec![1, 2, 3])), [0x43, 1, 2, 3]);
    assert_eq!(
        decode::<Bytes>(&[0x43, 1, 2, 3]).unwrap(),
        Bytes(vec![1, 2, 3])
    );
    // cbor-x tags a Uint8Array 64 unless told not to; the tag is accepted.
    assert_eq!(
        decode::<Bytes>(&[0xd8, 0x40, 0x43, 1, 2, 3]).unwrap(),
        Bytes(vec![1, 2, 3])
    );
    assert_eq!(
        decode::<Bytes>(&[0x83, 1, 2, 3]).unwrap(),
        Bytes(vec![1, 2, 3])
    );
    assert!(
        decode::<Bytes>(&[0x63, b'a', b'b', b'c']).is_err(),
        "text is not bytes"
    );
}

#[test]
fn decode_refuses_trailing_bytes() {
    let mut two = encode(&Text { text: "a".into() });
    assert!(decode::<Text>(&two).is_ok());
    two.push(0xf6);
    assert!(decode::<Text>(&two).is_err());
}

#[test]
fn a_call_round_trips_through_group_op_and_payload() {
    let call = HostCall::Nodes(NodesOps::NodeCreate(NodeCreate {
        parent_id: "p".into(),
        name: "n".into(),
    }));
    let (group, op, bytes) = (call.group(), call.op(), call.encode_args());
    assert_eq!((group, op), (Group::Nodes, NodeCreate::OP));
    assert_eq!(HostCall::decode(group, op, &bytes).unwrap(), call);
    assert_eq!(call.capability(), Capability::NodesWrite);
    assert_eq!(
        HostCall::decode(Group::Nodes, 99, &bytes),
        Err(CallDecodeError::UnknownOp)
    );
    assert!(matches!(
        HostCall::decode(Group::Nodes, NodeGet::OP, &[0xff]),
        Err(CallDecodeError::Malformed(_))
    ));
}

#[test]
fn a_kv_op_needs_the_capability_of_its_backend() {
    let get = |backend| KvGet {
        backend,
        key: "k".into(),
    };
    assert_eq!(get(KvBackend::User).capability(), Capability::StorageUser);
    assert_eq!(get(KvBackend::Scope).capability(), Capability::StorageScope);
    assert_eq!(
        get(KvBackend::Node {
            node_id: "n".into()
        })
        .capability(),
        Capability::StorageNode
    );
    assert_eq!(get(KvBackend::Local).capability(), Capability::StorageLocal);
    let call = HostCall::Storage(StorageOps::KvGet(get(KvBackend::Local)));
    assert_eq!(call.capability(), Capability::StorageLocal);
    assert_eq!(
        BlobPut {
            bytes: Bytes::default()
        }
        .capability(),
        Capability::StorageBlob
    );
}

#[test]
fn ihttp_frames_keep_r4_4_variant_names_and_events_are_snake_case() {
    use citadel_app_sdk_types::AppEvent;
    let end = IhttpFrame::new(7, FrameKind::End);
    assert_eq!(encode(&end), b"\xa3\x61v\x01\x66stream\x07\x64kind\x63End");
    let credit = encode(&FrameKind::Credit { n: 1 });
    assert_eq!(credit, b"\xa1\x66Credit\xa1\x61n\x01");
    assert_eq!(
        encode(&AppEvent::ReplyReady { handle: 3 }),
        b"\xa1\x6breply_ready\xa1\x66handle\x03"
    );
    assert_eq!(IhttpConsumed::OP, 4);
}
