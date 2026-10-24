//! The topic envelope's overhead, measured on the ABI's own CBOR: the agent caps a topic's group
//! at `max_msg_bytes` plus this (and the ciphertext's own), so a body the host admits is one the
//! server relays. Too small, and a full body is refused by the server after the host said yes.

use citadel_app_sdk_types::calls::topic::{
    MAX_TOPIC_NAME_LEN, TOPIC_ENVELOPE_OVERHEAD, TOPIC_ENVELOPE_TAG, TOPIC_MAX_MSG_BYTES_CEILING,
};
use citadel_app_sdk_types::{encode, Bytes};
use serde::Serialize;

/// The envelope as the host writes it: these keys, in this order.
#[derive(Serialize)]
struct Envelope<'a> {
    t: &'a str,
    name: &'a str,
    body: Bytes,
}

fn overhead(name_len: usize, body_len: usize) -> usize {
    let name = "a".repeat(name_len);
    let body = Bytes(vec![0; body_len]);
    encode(&Envelope {
        t: TOPIC_ENVELOPE_TAG,
        name: &name,
        body,
    })
    .len()
        - body_len
}

#[test]
fn the_overhead_is_reached_by_a_longest_name_and_never_exceeded() {
    let ceiling = TOPIC_MAX_MSG_BYTES_CEILING as usize;
    let most = (1..=MAX_TOPIC_NAME_LEN)
        .flat_map(|n| [0, 1, 23, 24, 255, 256, 4096, ceiling].map(|b| overhead(n, b)))
        .max();
    assert_eq!(most, Some(TOPIC_ENVELOPE_OVERHEAD as usize));
    assert_eq!(
        overhead(MAX_TOPIC_NAME_LEN, ceiling),
        TOPIC_ENVELOPE_OVERHEAD as usize
    );
}
