//! TURN relay configuration a `PeerConnect` may carry, and the path a peer connection took.
//!
//! The agent is never configured with TURN credentials of its own: the UI obtains short-lived
//! ones from its workspace server (`GetIceServers`) and hands them over per attempt. `IceServer`
//! has the shape of that response (and of WebRTC's `RTCIceServer`), so the list can be passed
//! through unchanged.

use custom_debug::Debug;
use serde::{Deserialize, Serialize};

#[cfg(feature = "typescript")]
use ts_rs::TS;

/// When the relay is used for one P2P attempt. Both peers must send the same policy.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum TurnPolicy {
    /// Try the direct (hole-punched) path first; relay only if it is impossible or fails.
    Fallback,
    /// Never attempt the direct path: the peers never learn each other's addresses.
    RelayOnly,
}

/// One `RTCIceServer` entry. `credential` is a TURN password: redacted from `Debug`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct IceServer {
    pub urls: Vec<String>,
    pub username: Option<String>,
    #[debug(with = secret_opt_debug_fmt)]
    pub credential: Option<String>,
}

fn secret_opt_debug_fmt(value: &Option<String>, f: &mut std::fmt::Formatter) -> std::fmt::Result {
    match value {
        Some(_) => write!(f, "Some(<redacted>)"),
        None => write!(f, "None"),
    }
}

/// The TURN relay configuration for one `PeerConnect` attempt.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct PeerTurnConfig {
    pub policy: TurnPolicy,
    pub ice_servers: Vec<IceServer>,
    /// Unix seconds after which the credentials are no longer valid. An expired config is
    /// treated as no config at all.
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub expires_at: u64,
}

/// The network path a peer connection's traffic takes, settled before success is reported.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum P2pPathReport {
    /// A direct (hole-punched) connection between the peers.
    Direct,
    /// Through a TURN relay; the Citadel server never carries the traffic.
    Turn,
    /// No P2P connection: relayed through the Citadel server, with no datagram channel.
    ServerRelay,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_credential_is_never_printed() {
        let config = PeerTurnConfig {
            policy: TurnPolicy::RelayOnly,
            ice_servers: vec![IceServer {
                urls: vec!["turns:turn.example:443?transport=tcp".to_string()],
                username: Some("user".to_string()),
                credential: Some("hunter2-turn-password".to_string()),
            }],
            expires_at: 1,
        };
        let printed = format!("{config:?} {config:#?}");
        assert!(!printed.contains("hunter2"), "{printed}");
        assert!(printed.contains("<redacted>"), "{printed}");
    }

    #[test]
    fn wire_names_are_snake_case() {
        let policy = |p: TurnPolicy| serde_json::to_string(&p).unwrap();
        let path = |p: P2pPathReport| serde_json::to_string(&p).unwrap();
        assert_eq!(policy(TurnPolicy::Fallback), r#""fallback""#);
        assert_eq!(policy(TurnPolicy::RelayOnly), r#""relay_only""#);
        assert_eq!(path(P2pPathReport::Direct), r#""direct""#);
        assert_eq!(path(P2pPathReport::Turn), r#""turn""#);
        assert_eq!(path(P2pPathReport::ServerRelay), r#""server_relay""#);
    }

    /// A client that predates `turn` sends PeerConnect without the key; it must still parse.
    #[test]
    fn a_peer_connect_without_turn_still_deserializes() {
        let request = crate::InternalServiceRequest::PeerConnect {
            request_id: uuid::Uuid::new_v4(),
            cid: 1,
            peer_cid: 2,
            udp_mode: Default::default(),
            session_security_settings: Default::default(),
            peer_session_password: None,
            turn: None,
        };
        let mut json = serde_json::to_value(&request).unwrap();
        let fields = json["PeerConnect"].as_object_mut().unwrap();
        assert!(fields.remove("turn").is_some(), "turn was not serialized");
        let parsed: crate::InternalServiceRequest = serde_json::from_value(json).unwrap();
        let crate::InternalServiceRequest::PeerConnect { turn, peer_cid, .. } = parsed else {
            panic!("parsed into another variant")
        };
        assert_eq!((turn, peer_cid), (None, 2));
    }

    #[test]
    fn a_peer_connect_with_turn_round_trips_in_the_agreed_shape() {
        let json = serde_json::json!({
            "policy": "relay_only",
            "ice_servers": [{"urls": ["turn:t:3478?transport=udp"], "username": "u", "credential": "c"}],
            "expires_at": 1_800_000_000u64,
        });
        let parsed: PeerTurnConfig = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(parsed.policy, TurnPolicy::RelayOnly);
        assert_eq!(serde_json::to_value(&parsed).unwrap(), json);
    }
}
