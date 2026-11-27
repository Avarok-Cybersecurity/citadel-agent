//! Byte fields reach the page as a `Uint8Array` (wire_bytes.rs), and the agent's wire does not
//! change by one byte: an agent and a page of different builds must keep talking.
//!
//! Each frame below is what the agent's socket carries for a byte-carrying family, written as
//! serde_json writes it. Reading it and writing it back must give the same text, on this build
//! and on the build before `wire_bytes` (the frames were checked against both).

use crate::{
    wire_bytes, AppKvResult, AttachProof, InternalServicePayload, SecurityKeyCeremony,
    SignInCredential,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_test::{assert_de_tokens, assert_de_tokens_error, assert_ser_tokens, Token};
use std::collections::HashMap;

const ID: &str = "6a1f2b3c-0000-4000-8000-000000000001";

fn frames() -> Vec<String> {
    [
        r#"{"Response":{"MessageNotification":{"message":[0,1,127,128,255],"cid":7,"peer_cid":9,"request_id":null}}}"#,
        r#"{"Response":{"MessageNotification":{"message":[],"cid":7,"peer_cid":9,"request_id":null}}}"#,
        r#"{"Response":{"MediaFrameNotification":{"cid":7,"peer_cid":9,"track":1,"kind":2,"sequence":3,"timestamp":4,"flags":5,"payload":[9,8,7],"request_id":null}}}"#,
        r#"{"Response":{"GroupMessageNotification":{"cid":7,"peer_cid":9,"message":[104,105],"group_key":{"cid":7,"mgid":11},"request_id":null}}}"#,
        r#"{"Response":{"LocalDBGetAllKVSuccess":{"cid":7,"peer_cid":null,"map":{"k":[1,2]},"request_id":null}}}"#,
        r#"{"Response":{"ReadPulledFileChunkSuccess":{"cid":7,"request_id":null,"total_size":10,"offset":0,"data":[1,2,3]}}}"#,
        r#"{"Request":{"Message":{"request_id":"ID","message":[1,2,3],"cid":7,"peer_cid":9,"security_level":"Standard"}}}"#,
        r#"{"Request":{"SendReliable":{"request_id":"ID","cid":7,"peer_cid":9,"message":[1,2],"security_level":"Standard","compression_hint":null}}}"#,
        r#"{"Request":{"MediaSend":{"request_id":"ID","cid":7,"peer_cid":9,"track":1,"kind":2,"timestamp":3,"flags":0,"payload":[255,0]}}}"#,
        r#"{"Request":{"SendFile":{"request_id":"ID","source":{"ByteContents":{"file_name":"a.txt","data":[97]}},"cid":7,"peer_cid":9,"chunk_size":null,"transfer_type":"FileTransfer"}}}"#,
        r#"{"Request":{"StageUploadChunk":{"request_id":"ID","cid":7,"upload_id":"ID","file_name":"a.bin","total_size":2,"offset":0,"data":[1,2]}}}"#,
        r#"{"Request":{"GroupMessage":{"cid":7,"message":[1],"group_key":{"cid":7,"mgid":11},"request_id":"ID"}}}"#,
        r#"{"Request":{"LocalDBSetKV":{"request_id":"ID","cid":7,"peer_cid":null,"key":"k","value":[0]}}}"#,
    ]
    .iter()
    .map(|frame| frame.replace("\"ID\"", &format!("\"{ID}\"")))
    .collect()
}

fn written_back<T: Serialize + DeserializeOwned>(frame: &str) -> String {
    let value: T = serde_json::from_str(frame).unwrap_or_else(|err| panic!("{frame}: {err}"));
    serde_json::to_string(&value).unwrap()
}

#[test]
fn every_family_s_socket_frame_is_written_back_byte_for_byte() {
    for frame in frames() {
        assert_eq!(written_back::<InternalServicePayload>(&frame), frame);
    }
}

#[test]
fn the_tcp_codec_carries_every_frame_unchanged() {
    for frame in frames() {
        let payload: InternalServicePayload = serde_json::from_str(&frame).unwrap();
        let binary = bincode2::serialize(&payload).unwrap();
        let back: InternalServicePayload = bincode2::deserialize(&binary).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), frame);
    }
}

#[test]
fn the_containers_keep_their_frames_too() {
    let cases: [(&str, fn(&str) -> String); 6] = [
        (r#"{"Value":[1,2]}"#, written_back::<AppKvResult>),
        (r#"{"Value":null}"#, written_back::<AppKvResult>),
        (r#"{"Token":[1,2]}"#, written_back::<AttachProof>),
        (
            r#"{"Authenticate":{"credential_ids":[[1,2],[3]]}}"#,
            written_back::<SecurityKeyCeremony>,
        ),
        (
            r#"{"id":1,"kind":"SecurityKey","label":"key","credential_id":[1,2],"created_ms":5,"last_used_ms":null,"consumed":false}"#,
            written_back::<SignInCredential>,
        ),
        (
            r#"{"id":1,"kind":"Password","label":"pw","credential_id":null,"created_ms":5,"last_used_ms":null,"consumed":false}"#,
            written_back::<SignInCredential>,
        ),
    ];
    for (frame, write_back) in cases {
        assert_eq!(write_back(frame), frame);
    }
}

#[test]
fn a_missing_optional_byte_field_still_reads_as_none() {
    let frame = r#"{"id":1,"kind":"Password","label":"pw","created_ms":5,"last_used_ms":null,"consumed":false}"#;
    let credential: SignInCredential = serde_json::from_str(frame).unwrap();
    assert_eq!(credential.credential_id, None);
}

#[test]
fn a_byte_field_written_as_a_string_is_refused_as_before() {
    let frame = r#"{"Response":{"MessageNotification":{"message":"hi","cid":7,"peer_cid":9,"request_id":null}}}"#;
    assert!(serde_json::from_str::<InternalServicePayload>(frame).is_err());
}

/// The same fields with and without `wire_bytes`: the attribute changes nothing either wire
/// carries, only the shape a byte-aware serializer (serde-wasm-bindgen) is handed.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct Plain {
    one: Vec<u8>,
    maybe: Option<Vec<u8>>,
    many: Vec<Vec<u8>>,
    keyed: HashMap<String, Vec<u8>>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct Typed {
    #[serde(with = "wire_bytes")]
    one: Vec<u8>,
    #[serde(default, with = "wire_bytes::option")]
    maybe: Option<Vec<u8>>,
    #[serde(with = "wire_bytes::seq")]
    many: Vec<Vec<u8>>,
    #[serde(with = "wire_bytes::map")]
    keyed: HashMap<String, Vec<u8>>,
}

fn pair(maybe: Option<Vec<u8>>) -> (Plain, Typed) {
    let keyed = HashMap::from([("k".to_string(), vec![4, 5])]);
    (
        Plain {
            one: vec![0, 255],
            maybe: maybe.clone(),
            many: vec![vec![1], vec![]],
            keyed: keyed.clone(),
        },
        Typed {
            one: vec![0, 255],
            maybe,
            many: vec![vec![1], vec![]],
            keyed,
        },
    )
}

#[test]
fn json_and_bincode_bytes_are_identical_with_and_without_the_attribute() {
    for maybe in [Some(vec![3]), None] {
        let (plain, typed) = pair(maybe);
        let json = serde_json::to_string(&plain).unwrap();
        assert_eq!(serde_json::to_string(&typed).unwrap(), json);
        assert_eq!(serde_json::from_str::<Typed>(&json).unwrap(), typed);
        let binary = bincode2::serialize(&plain).unwrap();
        assert_eq!(bincode2::serialize(&typed).unwrap(), binary);
        assert_eq!(bincode2::deserialize::<Typed>(&binary).unwrap(), typed);
    }
}

#[test]
fn a_byte_aware_serializer_is_handed_bytes_not_a_list_of_numbers() {
    let (_, typed) = pair(Some(vec![3]));
    assert_ser_tokens(
        &typed,
        &[
            Token::Struct {
                name: "Typed",
                len: 4,
            },
            Token::Str("one"),
            Token::Bytes(&[0, 255]),
            Token::Str("maybe"),
            Token::Some,
            Token::Bytes(&[3]),
            Token::Str("many"),
            Token::Seq { len: Some(2) },
            Token::Bytes(&[1]),
            Token::Bytes(&[]),
            Token::SeqEnd,
            Token::Str("keyed"),
            Token::Map { len: Some(1) },
            Token::Str("k"),
            Token::Bytes(&[4, 5]),
            Token::MapEnd,
            Token::StructEnd,
        ],
    );
}

#[derive(Deserialize, Debug, PartialEq)]
struct One(#[serde(with = "wire_bytes")] Vec<u8>);

#[test]
fn a_byte_buffer_and_a_list_of_numbers_both_decode() {
    assert_de_tokens(
        &One(vec![1, 2]),
        &[
            Token::NewtypeStruct { name: "One" },
            Token::ByteBuf(&[1, 2]),
        ],
    );
    assert_de_tokens(
        &One(vec![1, 2]),
        &[
            Token::NewtypeStruct { name: "One" },
            Token::Seq { len: Some(2) },
            Token::U8(1),
            Token::U8(2),
            Token::SeqEnd,
        ],
    );
}

#[test]
fn a_string_and_borrowed_text_bytes_are_refused() {
    assert_de_tokens_error::<One>(
        &[Token::NewtypeStruct { name: "One" }, Token::Str("hi")],
        "invalid type: string \"hi\", expected an array of bytes",
    );
    assert_de_tokens_error::<One>(
        &[
            Token::NewtypeStruct { name: "One" },
            Token::BorrowedBytes(b"hi"),
        ],
        "invalid type: byte array, expected an array of bytes",
    );
}
