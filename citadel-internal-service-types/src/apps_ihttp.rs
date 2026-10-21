//! Internal HTTP between Citadel apps over a P2P session, through the agent.
//!
//! A window hands the agent one encoded `citadel-ihttp-types` frame with `IhttpSend`; the agent
//! validates it, wraps it in the `citadel-ihttp/1` envelope and sends it through the account's
//! ILM, but only to a peer whose ILM advertised internal HTTP. A frame that arrives is validated
//! the same way and handed, as `IhttpFrameNotification`, only to windows that declared
//! `ClientCapabilities::ihttp`. The agent never reads the HTTP inside: routes, streams, credit
//! and status are the app hosts' business at either end.

use custom_debug::Debug;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg(feature = "typescript")]
use ts_rs::TS;

/// `IhttpSend` is with the account's ILM, which retransmits it until the peer acknowledges.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct IhttpSendAccepted {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub peer_cid: u64,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum IhttpSendRefusal {
    /// Not one valid frame of this version, within the frame limits.
    Malformed,
    /// The peer's ILM has not advertised internal HTTP (an older build, or no advertisement
    /// heard yet): it could not read the frame, so it is not sent one.
    NotAdvertised,
    /// The agent does not host this account's messaging; declare `agent_ilm` first.
    NotHosted,
    /// The account's ILM refused the message.
    Queue,
    /// The session is not this connection's, or is not signed in.
    Session,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct IhttpSendFailure {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub peer_cid: u64,
    pub request_id: Option<Uuid>,
    pub refusal: IhttpSendRefusal,
}

/// A valid frame from `peer_cid`, the authenticated session's peer, for this account's app host.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct IhttpFrameNotification {
    /// The recipient: route by this, as for `MessageNotification`.
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub peer_cid: u64,
    /// One CBOR `IhttpFrame`, already validated.
    #[cfg_attr(feature = "typescript", ts(type = "number[]"))]
    #[debug(with = crate::plaintext_debug_fmt)]
    pub frame: Vec<u8>,
    pub request_id: Option<Uuid>,
}
