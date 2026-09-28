//! A new agent and an old one, talking over a real SDK link, both ways.
//!
//! The old side is a messenger built with `IlmOptions::LEGACY`, which the wire
//! tests prove is byte-identical to a build that predates the extensions. But
//! this build's DECODER understands extended frames, so a legacy messenger
//! here would silently cope with something a real old peer could not. The
//! check is therefore on the raw bytes the old side's agent delivered: every
//! one of them must decode with a replica of the enum old builds use. A
//! compressed or piggybacked frame would not, and would have been handed to
//! the old UI as a plain message.

#![cfg(all(
    not(target_arch = "wasm32"),
    feature = "compression-brotli",
    feature = "compression-deflate"
))]

use citadel_internal_service_test_common as common;

use citadel_internal_service_connector::messenger::{
    CompressionHint, DynamicCompression, IlmOptions, InternalMessage,
};
use citadel_io::tokio;
use serde::Deserialize;
use std::sync::Arc;

mod traffic_support;
use traffic_support::{expect, fixtures, send, two_agents, Tally};

/// `WireWrapper` as every build before the extensions has it.
#[derive(Deserialize)]
#[allow(dead_code)]
enum Legacy {
    Message {
        source: u64,
        destination: u64,
        message_id: u64,
        contents: Vec<u8>,
    },
    ISMAux {
        signal: Box<InternalMessage>,
    },
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_agent_never_sends_an_old_one_a_frame_it_cannot_read() {
    common::setup_log();
    let new = IlmOptions {
        piggyback_acks: true,
        dynamic_compression: DynamicCompression::All,
    };
    let tally = Arc::new(Tally::default());
    let (mut fresh, mut old) = two_agents(new, IlmOptions::LEGACY, &tally).await;

    // Payloads that WOULD be compressed toward a capable peer, and a
    // conversation that WOULD carry piggybacked ACKs.
    let updates = fixtures("yjs-inc__");
    for update in &updates {
        send(&fresh.tx, old.cid, update, CompressionHint::YjsUpdate).await;
        expect(&mut old.rx, fresh.cid, update).await;
        send(&old.tx, fresh.cid, update, CompressionHint::YjsUpdate).await;
        expect(&mut fresh.rx, old.cid, update).await;
    }
    let snapshot = fixtures("yjs-snap__").remove(0);
    send(&fresh.tx, old.cid, &snapshot, CompressionHint::YjsUpdate).await;
    expect(&mut old.rx, fresh.cid, &snapshot).await;

    let arrived = old.arrived.lock().expect("lock").clone();
    assert!(
        arrived.len() > updates.len(),
        "the old side saw only {} frames; the check below proves nothing",
        arrived.len()
    );
    let unreadable = arrived
        .iter()
        .filter(|bytes| bincode2::deserialize::<Legacy>(bytes).is_err())
        .count();
    assert_eq!(
        unreadable,
        0,
        "{unreadable} of {} frames reaching the old agent would have been forwarded to its UI as garbage",
        arrived.len()
    );
}
