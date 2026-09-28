//! ILM's local half: handing a delivered message to whoever consumes it.
//!
//! Moved verbatim from mod.rs, then made generic over [`DeliveryTarget`] so the
//! agent's ILM host can deliver through a session's route while the browser's
//! messenger keeps its channel. One rule for what is deliverable, two targets.
// Same reason, and the same module-scoped allow, as mod.rs: ILM's error types.
#![allow(clippy::result_large_err)]

use crate::messenger::WrappedMessage;
use async_trait::async_trait;
use citadel_internal_service_types::{InternalServicePayload, InternalServiceResponse};
use citadel_io::tokio::sync::mpsc::UnboundedSender;
use intersession_layer_messaging::{DeliveryError, MessageMetadata};

/// Where a delivered message's `InternalServiceResponse` goes.
///
/// `Err` means nobody received it. ILM then keeps the message in its inbound
/// store with the frontier unmoved and sends no ACK, so the next cycle delivers
/// it again: a target that cannot tell "sent" from "nobody listening" would
/// have ILM acknowledge messages that reached no one.
pub trait DeliveryTarget: Send + Sync + 'static {
    fn deliver_response(&self, response: InternalServiceResponse) -> Result<(), DeliveryError>;
}

/// The browser's target: the messenger's channel to JavaScript.
impl DeliveryTarget for UnboundedSender<InternalServiceResponse> {
    fn deliver_response(&self, response: InternalServiceResponse) -> Result<(), DeliveryError> {
        self.send(response)
            .map_err(|_| DeliveryError::ChannelClosed)
    }
}

pub struct LocalDeliveryTx<T: DeliveryTarget = UnboundedSender<InternalServiceResponse>> {
    final_tx: T,
}

impl<T: DeliveryTarget> LocalDeliveryTx<T> {
    /// Delivered messages go to `final_tx` as the `InternalServiceResponse`
    /// they carry. Shared by the browser's messenger and the agent's ILM host,
    /// so both apply the same rule to what counts as deliverable.
    pub fn new(final_tx: T) -> Self {
        Self { final_tx }
    }
}

#[async_trait]
impl<T: DeliveryTarget> intersession_layer_messaging::local_delivery::LocalDelivery<WrappedMessage>
    for LocalDeliveryTx<T>
{
    async fn deliver(&self, message: WrappedMessage) -> Result<(), DeliveryError> {
        let msg_id = message.message_id();
        let InternalServicePayload::Response(response) = message.contents else {
            // Logged, not silent: ILM records this as a delivery failure but the
            // reason never reached anyone.
            ::log::info!(target: "ism", "[ILM-DELIVER] msg_id={msg_id} REJECTED: payload was not a Response");
            return Err(DeliveryError::BadInput);
        };

        // The join key the reconnect-loss investigation has been missing.
        //
        // ILM logs msg_id with no content; the client logs content with no
        // msg_id, so no one can say which delivery carried which message. Length
        // cannot bridge them either — the offline test's three messages are the
        // same length and differ by one digit. This prints a content
        // fingerprint next to the id; the client prints the same fingerprint on
        // receipt, and the two sides finally line up.
        //
        // Deliberately target: "ism". The messenger's existing `target: "citadel"`
        // lines do not appear in these runs at all, whereas every ILM line does.
        // `log_enabled!` FIRST, before the loop.
        //
        // The fingerprint is FNV-1a over the WHOLE payload -- three operations
        // per byte -- and it was computed unconditionally, then handed to a
        // macro that discards it whenever the `ism` target is filtered out,
        // which is every deployment that is not this test run. A 1 MiB document
        // update paid 1,048,576 iterations per delivery, on the delivery path,
        // for no output.
        //
        // The UI hit exactly this and fixed it: `debugLog` is a noop in
        // production but its ARGUMENTS are still evaluated, so `fnv1a64` ran
        // over every inbound message. That fix never crossed into Rust, where
        // `log::info!` has the same property. A correct fix applied in one of
        // the places its mechanism appears is this repository's most common
        // defect, and this was one of them.
        if ::log::log_enabled!(target: "ism", ::log::Level::Info) {
            if let InternalServiceResponse::MessageNotification(n) = &response {
                let mut fp: u64 = 0xcbf2_9ce4_8422_2325;
                for b in &n.message {
                    fp ^= *b as u64;
                    fp = fp.wrapping_mul(0x100_0000_01b3);
                }
                // Field ORDER matters here, not just content. CI truncates console
                // lines around 348 chars, and two 20-digit CIDs ahead of the
                // fingerprint meant every one of these lines was cut off exactly at
                // `len=` — 26 of them logged, not one readable. The join key goes
                // first, and the CIDs are trimmed to their last 6 digits, which is
                // plenty to tell two peers apart in one run.
                ::log::info!(
                    target: "ism",
                    "[ILM-DELIVER] fp={:016x} msg_id={msg_id} len={} cid=..{} peer=..{}",
                    fp,
                    n.message.len(),
                    n.cid % 1_000_000,
                    n.peer_cid % 1_000_000
                );
            }
        }

        self.final_tx.deliver_response(response)
    }
}
