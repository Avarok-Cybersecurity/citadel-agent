// This module and its children implement intersession_layer_messaging's `Backend`
// trait, whose error type is fixed as `BackendError<T>`.
// `BackendError::SendFailed` carries the whole undelivered message inline, so
// with T = WrappedMessage the Err variant is ~288 bytes and clippy's
// result_large_err (128-byte threshold) fires on all 26 methods.
//
// Boxing is not available here: the return type is dictated by the trait, so the
// only way to satisfy the lint is to change ILM's public error enum, which every
// consumer of that crate would have to follow. That is a far wider change than
// the lint is worth, and it would not make anything faster — these are async
// trait methods whose futures are boxed already.
//
// Scoped to this module rather than the crate root so a genuinely oversized
// error elsewhere in the connector still gets caught.
#![allow(clippy::result_large_err)]

//! What every ILM host shares: the message type ILM carries, its storage
//! (`backend`) and its wire format (`wire`). The agent hosts each account's ILM
//! (citadel-internal-service `kernel/ilm`); nothing else does.

use citadel_internal_service_types::InternalServicePayload;
use intersession_layer_messaging::{MessageMetadata, Payload};
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub mod backend;
pub mod backend_channel;
pub mod backend_map;
pub mod stored_map;
pub mod wire;

pub use intersession_layer_messaging::{CompressionHint, DynamicCompression, IlmOptions};

/// What an ILM hosted for a workspace account spends on the wire: both traffic
/// reductions, each used toward a peer only once that peer advertised it.
///
/// The agent's ILM (citadel-internal-service `kernel/ilm`) builds on it.
///
/// It does not advertise internal HTTP: only the agent reads `citadel-ihttp/1`
/// envelopes, validating each one before a window sees it, so only the
/// agent's ILM may say it does (citadel-internal-service `kernel/apps/ihttp`).
pub const ACCOUNT_ILM_OPTIONS: IlmOptions = IlmOptions {
    piggyback_acks: true,
    dynamic_compression: DynamicCompression::All,
    internal_http: false,
};
pub use wire::WireWrapper;

/// Platform-agnostic async sleep function
/// - Native: Uses tokio::time::sleep
/// - WASM: Uses wasmtimer::tokio::sleep
#[cfg(not(target_arch = "wasm32"))]
pub async fn sleep_internal(duration: Duration) {
    citadel_io::tokio::time::sleep(duration).await;
}

#[cfg(target_arch = "wasm32")]
pub async fn sleep_internal(duration: Duration) {
    wasmtimer::tokio::sleep(duration).await;
}

pub type InternalMessage = Payload<WrappedMessage>;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WrappedMessage {
    source_id: u64,
    destination_id: u64,
    message_id: u64,
    contents: InternalServicePayload,
}

impl MessageMetadata for WrappedMessage {
    type PeerId = u64;
    type MessageId = u64;
    type Contents = InternalServicePayload;

    fn source_id(&self) -> Self::PeerId {
        self.source_id
    }

    fn destination_id(&self) -> Self::PeerId {
        self.destination_id
    }

    fn message_id(&self) -> Self::MessageId {
        self.message_id
    }

    fn contents(&self) -> &Self::Contents {
        &self.contents
    }

    fn construct_from_parts(
        source_id: Self::PeerId,
        destination_id: Self::PeerId,
        message_id: Self::MessageId,
        contents: impl Into<Self::Contents>,
    ) -> Self {
        Self {
            source_id,
            destination_id,
            message_id,
            contents: contents.into(),
        }
    }

    /// Seeded from the wall clock, not from zero.
    ///
    /// This backend's store is the agent's LocalDB, keyed by CID. A user on a
    /// new device, or one who has cleared site data, arrives with the same CID
    /// and an EMPTY store -- while the peer's durable delivery frontier for
    /// them is untouched. Re-minting from zero put every message at or below
    /// that frontier, where `safe_to_ack` calls it a duplicate: re-ACKed,
    /// cleared, and never delivered. Nothing errored on either side.
    ///
    /// Microseconds, because the seed must clear the HIGHEST id the previous
    /// incarnation reached, which is its own seed plus however many messages it
    /// sent -- see `platform_timestamp_micros`.
    fn initial_message_id() -> Self::MessageId {
        intersession_layer_messaging::platform_timestamp_micros()
    }
}
