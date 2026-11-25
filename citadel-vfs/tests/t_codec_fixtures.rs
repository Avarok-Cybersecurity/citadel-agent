//! `fixtures/codec`: the encoding of each op, its hashes and the bytes to sign; the refusals;
//! the derived ids.

mod common;

use citadel_vfs::cbor::{decode, Value};
use citadel_vfs::op::{op_hash, op_message, Body, Header, Op};
use citadel_vfs::{sha3_256, to_hex, InodeId, Namespace, VolumeId};
use common::{build, hex_vec, parse, J};

fn hex_of(j: &J, key: &str) -> Vec<u8> {
    hex_vec(j[key].as_str().unwrap_or_else(|| panic!("{key}")))
}

fn op_case(text: &str) {
    let f = parse(text);
    let name = f["name"].as_str().unwrap_or_default();
    let header_bytes = hex_of(&f["header"], "cbor_hex");
    let body_bytes = hex_of(&f["body"], "cbor_hex");
    assert_eq!(
        build::value(&f["header"]["model"]).encode(),
        header_bytes,
        "{name}: header model encodes to cbor_hex"
    );
    assert_eq!(
        build::value(&f["body"]["model"]).encode(),
        body_bytes,
        "{name}: body model encodes to cbor_hex"
    );
    let header = Header::decode(&header_bytes).unwrap_or_else(|e| panic!("{name}: header: {e:?}"));
    assert_eq!(header.encode(), header_bytes, "{name}: header round trip");
    let body =
        Body::decode(header.kind, &body_bytes).unwrap_or_else(|e| panic!("{name}: body: {e:?}"));
    assert_eq!(body.encode(), body_bytes, "{name}: body round trip");
    assert_eq!(
        to_hex(&sha3_256(&[&body_bytes])),
        f["body_hash_hex"],
        "{name}: body_hash"
    );
    assert_eq!(
        to_hex(&header.body_hash),
        f["body_hash_hex"],
        "{name}: header carries body_hash"
    );
    assert_eq!(
        to_hex(&op_hash(&header_bytes)),
        f["op_hash_hex"],
        "{name}: op_hash"
    );
    assert_eq!(
        to_hex(&op_message(&header_bytes)),
        f["sign_message_hex"],
        "{name}: sign message"
    );
    let op = Op::decode(&header_bytes, &body_bytes).unwrap_or_else(|e| panic!("{name}: op: {e:?}"));
    assert_eq!(op.body.as_ref(), Some(&body), "{name}: op body");
}

fn refused(text: &str) {
    let f = parse(text);
    let raw = hex_of(&f, "cbor_hex");
    assert!(
        decode(&raw).is_err(),
        "{}: decoded {:?}",
        f["name"],
        decode(&raw)
    );
    assert!(Header::decode(&raw).is_err(), "{}: a header", f["name"]);
}

both! {
    fn c01() { op_case(include_str!("contracts/rel-1124/codec/01-create-dir.json")) }
    fn c02() { op_case(include_str!("contracts/rel-1124/codec/02-create-file.json")) }
    fn c03() { op_case(include_str!("contracts/rel-1124/codec/03-set-pointee-chained.json")) }
    fn c04() { op_case(include_str!("contracts/rel-1124/codec/04-remove.json")) }
    fn c05() { op_case(include_str!("contracts/rel-1124/codec/05-server-create.json")) }
    fn c06() { op_case(include_str!("contracts/rel-1124/codec/06-admit-member.json")) }
    fn c07() { op_case(include_str!("contracts/rel-1124/codec/07-dismiss.json")) }
    fn c08() { op_case(include_str!("contracts/rel-1124/codec/08-guest-set-pointee.json")) }
    fn nonshortest() { refused(include_str!("contracts/rel-1124/codec/invalid-nonshortest-lamport.json")) }
    fn null_prev() { refused(include_str!("contracts/rel-1124/codec/invalid-null-prev.json")) }
    fn unsorted() { refused(include_str!("contracts/rel-1124/codec/invalid-unsorted-header.json")) }

    fn chained_prev_is_the_previous_op_hash() {
        let two = parse(include_str!("contracts/rel-1124/codec/02-create-file.json"));
        let three = parse(include_str!("contracts/rel-1124/codec/03-set-pointee-chained.json"));
        let header = Header::decode(&hex_of(&three["header"], "cbor_hex")).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(header.prev.map(|p| to_hex(&p)), two["op_hash_hex"].as_str().map(String::from));
    }

    fn sign_kat_message() {
        let f = parse(include_str!("contracts/rel-1124/codec/sign-kat.json"));
        assert_eq!(to_hex(&op_message(&hex_of(&f, "header_hex"))), f["message_hex"]);
        assert_eq!(f["domain"], "citadel-vfs-op-v1");
    }

    fn inode_kat() {
        let f = parse(include_str!("contracts/rel-1124/codec/inode-kat.json"));
        let derived = f["derived"].as_array().cloned().unwrap_or_default();
        assert_eq!(derived.len(), 15);
        for d in &derived {
            let volume = VolumeId::parse(d["volume"].as_str().unwrap_or_default()).unwrap_or_else(|_| panic!("{d}"));
            let ns = Namespace::parse(d["namespace"].as_str().unwrap_or_default()).unwrap_or_else(|| panic!("{d}"));
            let id = InodeId::derive(&volume, ns, d["key"].as_str().unwrap_or_default()).unwrap_or_else(|_| panic!("{d}"));
            assert_eq!(id.to_hex(), d["inode_hex"], "{d}");
            if ns == Namespace::Root {
                assert_eq!(InodeId::root(&volume), id, "{d}");
            }
        }
    }

    fn values_refuse_trailing_bytes() {
        let mut raw = Value::Uint(1).encode();
        raw.push(0);
        assert!(decode(&raw).is_err());
    }
}
