//! What a P2P connection's key exchange proved about the peer, on the UI wire.
//!
//! Each end of a P2P key exchange signs its stage with its account's identity key, and the other
//! end checks it against the key it pinned for that peer the first time it saw one (the SDK's
//! `proto::peer::kex_auth`). Only public data crosses this wire: fingerprints of identity keys
//! (16 base32 characters, shown `abcd-efgh-ijkl-mnop`), never a key's secret half.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg(feature = "typescript")]
use ts_rs::TS;

/// Whether a peer connection was signed by the identity key pinned for the peer.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum PeerIdentityMark {
    /// Signed by the peer's pinned identity key, whose fingerprint this is.
    Verified { fingerprint: String },
    /// Not signed: the peer runs an older agent, or its account has no identity key.
    Unverified,
}

/// Both ends' fingerprints, for the person to compare with the peer out of band.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct PeerIdentityInfo {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub peer_cid: u64,
    /// This account's identity fingerprint; `None` for an account with no identity key.
    pub own_fingerprint: Option<String>,
    /// The fingerprint pinned for the peer; `None` until the peer has signed an exchange.
    pub peer_fingerprint: Option<String>,
    /// A key the peer signed with since, other than the pinned one, waiting for the person to
    /// accept it. While it waits, every exchange with the peer is refused.
    pub pending_fingerprint: Option<String>,
    pub request_id: Option<Uuid>,
}

/// A peer signed a key exchange with an identity key other than the one pinned for it. The
/// exchange was refused, and so is every later one until the person accepts the new key with
/// `AcceptPeerIdentityKey`. Unsolicited: route it by `cid`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct PeerIdentityKeyChanged {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub peer_cid: u64,
    pub pinned_fingerprint: String,
    pub presented_fingerprint: String,
    pub request_id: Option<Uuid>,
}

/// The person accepted the peer's new key: it is pinned, and the pair may connect.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct PeerIdentityKeyAccepted {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub peer_cid: u64,
    pub fingerprint: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct PeerIdentityFailure {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub peer_cid: u64,
    pub message: String,
    pub request_id: Option<Uuid>,
}
