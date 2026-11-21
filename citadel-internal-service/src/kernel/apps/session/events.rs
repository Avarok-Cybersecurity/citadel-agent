//! What the agent's session machinery says about an app session's account, read as app session
//! events. Pure.
//!
//! The account is an ordinary session in the agent's map, so its link reports what every
//! session's does (c2s_reader.rs, reconnect/): server frames as `MessageNotification`, a drop
//! as `ServerConnectionLost`, the way back as `ServerReconnected`, the end as
//! `ServerReconnectFailed` or `DisconnectNotification`. Its only subscriber is the app session's
//! own route, which turns each into one `AppSessionMessage` or `AppSessionState`.

use citadel_internal_service_types::{InternalServiceResponse, SessionStatus};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionEvent {
    Message(Vec<u8>),
    State(SessionStatus),
}

/// The `DisconnectNotification` reason: the account's session ended.
pub(crate) const DISCONNECTED: &str = "disconnected";

/// `response` about `server_cid`'s session as an event; `None` for anything else, including
/// what concerns another session or a peer.
pub(crate) fn translate(
    server_cid: u64,
    response: InternalServiceResponse,
) -> Option<SessionEvent> {
    match response {
        InternalServiceResponse::MessageNotification(m)
            if m.cid == server_cid && m.peer_cid == 0 =>
        {
            Some(SessionEvent::Message(m.message))
        }
        InternalServiceResponse::ServerConnectionLost(lost) if lost.cid == server_cid => {
            Some(SessionEvent::State(if lost.reconnecting {
                SessionStatus::Connecting
            } else {
                SessionStatus::Closed {
                    reason: DISCONNECTED.to_string(),
                }
            }))
        }
        InternalServiceResponse::ServerReconnected(back) if back.cid == server_cid => {
            Some(SessionEvent::State(SessionStatus::Connected))
        }
        InternalServiceResponse::ServerReconnectFailed(failed) if failed.cid == server_cid => {
            Some(SessionEvent::State(SessionStatus::Closed {
                reason: failed.reason,
            }))
        }
        InternalServiceResponse::DisconnectNotification(d)
            if d.cid == server_cid && d.peer_cid.is_none() =>
        {
            Some(SessionEvent::State(SessionStatus::Closed {
                reason: DISCONNECTED.to_string(),
            }))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    //! Control: drop the `m.cid == server_cid` guard → another session's frame becomes this
    //! session's message and `only_this_accounts_frames_are_its_messages` goes red.
    use super::*;
    use citadel_internal_service_types::{
        MessageNotification, ServerConnectionLost, ServerReconnected,
    };

    fn frame(cid: u64, peer_cid: u64) -> InternalServiceResponse {
        InternalServiceResponse::MessageNotification(MessageNotification {
            message: b"hi".to_vec(),
            cid,
            peer_cid,
            request_id: None,
        })
    }

    #[test]
    fn only_this_accounts_frames_are_its_messages() {
        assert_eq!(
            translate(5, frame(5, 0)),
            Some(SessionEvent::Message(b"hi".to_vec()))
        );
        assert_eq!(translate(5, frame(6, 0)), None, "another session's frame");
        assert_eq!(translate(5, frame(5, 9)), None, "a peer's message");
    }

    #[test]
    fn a_drop_is_connecting_and_the_way_back_connected() {
        let lost = |reconnecting| {
            InternalServiceResponse::ServerConnectionLost(ServerConnectionLost {
                cid: 5,
                reconnecting,
                request_id: None,
            })
        };
        assert_eq!(
            translate(5, lost(true)),
            Some(SessionEvent::State(SessionStatus::Connecting))
        );
        assert!(matches!(
            translate(5, lost(false)),
            Some(SessionEvent::State(SessionStatus::Closed { .. }))
        ));
        let back = InternalServiceResponse::ServerReconnected(ServerReconnected {
            cid: 5,
            request_id: None,
        });
        assert_eq!(
            translate(5, back),
            Some(SessionEvent::State(SessionStatus::Connected))
        );
    }
}
