//! bincode tags a `WireWrapper` variant by its index, so the indices are the
//! wire format: reordering the enum would make two builds misread each other.

use super::*;

fn tag(wrapper: &WireWrapper) -> u32 {
    let bytes = bincode2::serialize(wrapper).expect("serialize");
    u32::from_le_bytes(bytes[..4].try_into().expect("a tag"))
}

#[test]
fn each_variant_keeps_its_index() {
    let message = WireWrapper::Message {
        source: ALICE,
        destination: BOB,
        message_id: ID,
        contents: Vec::new(),
    };
    let control = WireWrapper::ISMAux {
        signal: Box::new(ack()),
    };
    let v2 = WireWrapper::MessageV2 {
        source: ALICE,
        destination: BOB,
        message_id: ID,
        codec: 0,
        dict_id: 0,
        piggybacked_ack: None,
        contents: Vec::new(),
    };
    assert_eq!([tag(&message), tag(&control), tag(&v2)], [0, 1, 2]);
}
