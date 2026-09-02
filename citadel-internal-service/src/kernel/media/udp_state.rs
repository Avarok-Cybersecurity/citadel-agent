//! Lifecycle of a peer connection's UDP transport.
//!
//! The SDK offers the UDP channel exactly once per peer connection, and
//! dropping its receive half tears the path down at the protocol level (its
//! Drop sends DisconnectUDP). Every transition below therefore either keeps the
//! halves reachable or deliberately declares the path gone — "accidentally
//! consumed" is not a representable state.
//!
//! "Per peer CONNECTION", not per peer. A simultaneous connect makes two of
//! them — `connect.rs` installs the initiator's offer and `PeerChannelCreated`
//! then arrives carrying a DIFFERENT channel, which its own comment records:
//! "this PeerChannelCreated event may carry a DIFFERENT channel that also needs
//! its stream consumed". Two sessions in one browser produce exactly that
//! shape, because the auto-connect service has both sides initiate.
//!
//! `Pending` therefore holds EVERY outstanding offer and the open races them.
//! It used to hold one and each new offer overwrote the last, so when the
//! surviving connection was not the one whose offer was kept, the receiver
//! never fired and the open failed with "no UDP channel within 5s" — every
//! time, for that topology.

use citadel_sdk::prelude::{OutboundUdpSender, PeerChannelRecvHalf, Ratchet, UdpChannel};
use tokio::sync::oneshot::Receiver as OneshotReceiver;

/// Where a peer connection's UDP transport currently lives.
///
/// This is a lifecycle, not a cache: states only move under the server
/// connection map's write lock, and the open handler's generation check decides
/// races between an awaited open and a close.
pub enum UdpState<R: Ratchet> {
    /// Offers the SDK has not yet delivered on; the first open races all of
    /// them. More than one exists whenever a simultaneous connect produced more
    /// than one peer connection — see the module header.
    Pending(Vec<OneshotReceiver<UdpChannel<R>>>),
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
            Some(rx) => Self::Pending(vec![rx]),
            None => Self::Unavailable,
        }
    }

    /// Record one more offer of a UDP channel for this peer.
    ///
    /// Accumulates rather than replaces. The SDK offers a channel once per peer
    /// CONNECTION, and a simultaneous connect makes two of them, so the second
    /// offer used to overwrite the first and drop it — and when the surviving
    /// connection was not the one whose offer was kept, the receiver never fired
    /// and every call to that peer failed with "no UDP channel within 5s".
    ///
    /// A state that already has a usable path (`Idle`, `Lent`) or an open
    /// in flight (`Opening`) is left alone: the caller checks those, and this
    /// keeps the rule in one place rather than two.
    pub fn offer(&mut self, rx: OneshotReceiver<UdpChannel<R>>) {
        match self {
            Self::Pending(offers) => offers.push(rx),
            _ => *self = Self::Pending(vec![rx]),
        }
    }

    /// How many offers are outstanding. Test seam.
    #[cfg(test)]
    pub fn pending_offers(&self) -> usize {
        match self {
            Self::Pending(offers) => offers.len(),
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use citadel_sdk::prelude::StackedRatchet;

    fn offer_rx() -> OneshotReceiver<UdpChannel<StackedRatchet>> {
        let (_tx, rx) = tokio::sync::oneshot::channel();
        // The sender is dropped, which is fine: these tests are about how many
        // offers are RETAINED, not about what any of them delivers.
        rx
    }

    #[test]
    fn a_second_offer_is_kept_alongside_the_first() {
        // The defect: a simultaneous connect produces two peer connections, the
        // SDK offers a channel once per connection, and keeping only the last
        // meant that if the surviving connection was not that one, no channel
        // ever arrived.
        let mut state: UdpState<StackedRatchet> = UdpState::from_optional_channel(Some(offer_rx()));
        assert_eq!(state.pending_offers(), 1);

        state.offer(offer_rx());

        assert_eq!(
            state.pending_offers(),
            2,
            "the second offer replaced the first instead of joining it"
        );
    }

    #[test]
    fn an_offer_after_unavailable_starts_a_fresh_set() {
        // A fresh handshake is the only thing that recovers from Unavailable,
        // and it must not be swallowed.
        let mut state: UdpState<StackedRatchet> = UdpState::Unavailable;

        state.offer(offer_rx());

        assert_eq!(state.pending_offers(), 1);
    }

    #[test]
    fn no_offer_means_unavailable_rather_than_an_empty_wait() {
        // The opposite failure: an empty Pending would make the open race
        // nothing at all and wait out the full timeout for a peer that never
        // offered UDP, instead of failing immediately with the true reason.
        let state: UdpState<StackedRatchet> = UdpState::from_optional_channel(None);

        assert!(matches!(state, UdpState::Unavailable));
        assert_eq!(state.pending_offers(), 0);
    }
}
