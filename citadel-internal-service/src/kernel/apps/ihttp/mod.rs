//! Internal HTTP relayed between a window and a peer, as chat messages are: through the
//! account's ILM, over the authenticated P2P session.
//!
//! The agent checks, at its boundary, two things and nothing else. Every frame in either
//! direction is one valid `citadel-ihttp-types` frame (a malformed one is dropped and
//! counted). And a frame goes only to a peer whose ILM advertised `INTERNAL_HTTP`, and only to
//! windows that declared `ClientCapabilities::ihttp`, so neither an older peer nor an older
//! window is ever handed one. What the HTTP inside means is the app hosts' business.

pub(crate) mod envelope;

#[cfg(test)]
mod tests;

use citadel_ihttp_types::decode;
use citadel_internal_service_types::{ClientCapabilities, IhttpSendRefusal};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use uuid::Uuid;

/// Frames the relay moved and dropped, since the agent started.
#[derive(Default)]
pub(crate) struct IhttpCounters {
    sent: AtomicU64,
    received: AtomicU64,
    malformed_in: AtomicU64,
    malformed_out: AtomicU64,
    not_advertised: AtomicU64,
    no_receiver: AtomicU64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct IhttpCounts {
    pub sent: u64,
    pub received: u64,
    pub malformed_in: u64,
    pub malformed_out: u64,
    pub not_advertised: u64,
    pub no_receiver: u64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Count {
    Sent,
    Received,
    MalformedIn,
    MalformedOut,
    NotAdvertised,
    NoReceiver,
}

impl IhttpCounters {
    pub(crate) fn add(&self, what: Count) {
        let counter = match what {
            Count::Sent => &self.sent,
            Count::Received => &self.received,
            Count::MalformedIn => &self.malformed_in,
            Count::MalformedOut => &self.malformed_out,
            Count::NotAdvertised => &self.not_advertised,
            Count::NoReceiver => &self.no_receiver,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn read(&self) -> IhttpCounts {
        let get = |c: &AtomicU64| c.load(Ordering::Relaxed);
        IhttpCounts {
            sent: get(&self.sent),
            received: get(&self.received),
            malformed_in: get(&self.malformed_in),
            malformed_out: get(&self.malformed_out),
            not_advertised: get(&self.not_advertised),
            no_receiver: get(&self.no_receiver),
        }
    }
}

/// The envelope to send for a window's frame, or why none is sent: a frame that is not one,
/// or a peer that has not advertised internal HTTP.
pub(crate) fn outbound(
    frame: &[u8],
    peer_carries_internal_http: bool,
    counters: &IhttpCounters,
) -> Result<Vec<u8>, IhttpSendRefusal> {
    let Ok(frame) = decode(frame) else {
        counters.add(Count::MalformedOut);
        return Err(IhttpSendRefusal::Malformed);
    };
    if !peer_carries_internal_http {
        counters.add(Count::NotAdvertised);
        return Err(IhttpSendRefusal::NotAdvertised);
    }
    let wrapped = envelope::wrap(&frame).map_err(|_| IhttpSendRefusal::Malformed)?;
    counters.add(Count::Sent);
    Ok(wrapped)
}

/// The attached connections that receive an arriving frame: those that declared `ihttp`.
pub(crate) fn receivers(
    attached: &[Uuid],
    capabilities: &HashMap<Uuid, ClientCapabilities>,
) -> Vec<Uuid> {
    attached
        .iter()
        .filter(|c| capabilities.get(c).is_some_and(|caps| caps.ihttp))
        .copied()
        .collect()
}

/// Whether bytes a window asked to send as an ordinary message are an internal-HTTP envelope,
/// which must go through `IhttpSend` and its checks instead.
pub(crate) fn smuggled(message: &[u8]) -> bool {
    envelope::is_envelope(message)
}
