//! Lifecycle of a peer connection's UDP transport.
//!
//! The SDK offers the UDP channel exactly once per peer connection, and
//! dropping its receive half tears the path down at the protocol level (its
//! Drop sends DisconnectUDP). Every transition below therefore either keeps the
//! halves reachable or deliberately declares the path gone — "accidentally
//! consumed" is not a representable state.

use citadel_sdk::prelude::{OutboundUdpSender, PeerChannelRecvHalf, Ratchet, UdpChannel};
use tokio::sync::oneshot::Receiver as OneshotReceiver;

/// Where a peer connection's UDP transport currently lives.
///
/// This is a lifecycle, not a cache: states only move under the server
/// connection map's write lock, and the open handler's generation check decides
/// races between an awaited open and a close.
pub enum UdpState<R: Ratchet> {
    /// The SDK has not yet delivered the channel; the first open waits on this.
    Pending(OneshotReceiver<UdpChannel<R>>),
    /// A first open is mid-await on the oneshot. Kept distinct from `Lent` so a
    /// concurrent open reports "in progress" instead of the misleading
    /// "UdpMode disabled", and so a raced close knows an open may still land.
    Opening,
    /// Split halves parked between calls, ready for the next open with no wait.
    Idle {
        tx: OutboundUdpSender,
        rx: PeerChannelRecvHalf<R>,
    },
    /// The receive half is inside a live session's pump. The send half is
    /// retained here (it is Clone) so the pair can be re-parked on close.
    Lent { tx: OutboundUdpSender },
    /// No usable UDP path: the peer connected with UdpMode disabled, the offer
    /// was withdrawn, or the stream ended. Only a fresh peer handshake
    /// (which installs a new `Pending`) can recover from this.
    Unavailable,
}

impl<R: Ratchet> UdpState<R> {
    pub fn from_optional_channel(udp_rx: Option<OneshotReceiver<UdpChannel<R>>>) -> Self {
        match udp_rx {
            Some(rx) => Self::Pending(rx),
            None => Self::Unavailable,
        }
    }
}
