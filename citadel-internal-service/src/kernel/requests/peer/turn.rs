//! Maps the TURN servers a `PeerConnect` / `PeerConnectAccept` carries onto the SDK's relay
//! configuration, and the SDK's settled path onto the wire report. The mapping is pure (the
//! caller supplies the clock); [`set_peer_turn`] is the one place it reaches the SDK.

use citadel_internal_service_types::{P2pPathReport, PeerTurnConfig, TurnPolicy as WirePolicy};
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::{
    NetworkError, NodeRemote, NodeRequest, P2pPath, Ratchet, SetPeerTurnConfig, TurnPolicy,
    TurnRelayConfig, TurnServerCredential, TurnTransport,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The relay configuration for one attempt, or `None` when the config is absent, expired, or
/// names no usable TURN server — each of which means "no relay".
///
/// Only `turn:` / `turns:` URLs are used (`stun:` is the hole puncher's business, not the
/// relay's). Servers are ordered UDP first, then TCP, then TLS, so `turns:…:443` — the one most
/// likely to cross a restrictive firewall and the slowest — is the last resort.
pub fn relay_config(turn: Option<&PeerTurnConfig>, now: SystemTime) -> Option<TurnRelayConfig> {
    let turn = turn?;
    let expires_at = UNIX_EPOCH + Duration::from_secs(turn.expires_at);
    if expires_at <= now {
        warn!(target: "citadel", "[PeerConnect] TURN config expired; connecting without a relay");
        return None;
    }

    let mut servers: Vec<TurnServerCredential> = turn
        .ice_servers
        .iter()
        .flat_map(|ice| ice.urls.iter().map(move |url| (ice, url)))
        .filter(|(_, url)| url.starts_with("turn:") || url.starts_with("turns:"))
        .filter_map(|(ice, url)| {
            let (Some(username), Some(credential)) = (&ice.username, &ice.credential) else {
                warn!(target: "citadel", "[PeerConnect] TURN URL {url} has no credential; skipped");
                return None;
            };
            TurnServerCredential::new(url, username, credential, Some(expires_at))
                .inspect_err(|err| warn!(target: "citadel", "[PeerConnect] skipped: {err}"))
                .ok()
        })
        .collect();

    if servers.is_empty() {
        warn!(target: "citadel", "[PeerConnect] TURN config names no usable TURN server; connecting without a relay");
        return None;
    }
    servers.sort_by_key(|s| transport_rank(s.url.transport));

    let policy = match turn.policy {
        WirePolicy::Fallback => TurnPolicy::Fallback,
        WirePolicy::RelayOnly => TurnPolicy::RelayOnly,
    };
    Some(TurnRelayConfig::new(servers, policy))
}

/// Sets (or, when `turn` maps to no relay, clears) the relay for the next P2P attempt between
/// `session_cid` and `peer_cid`. Must run before that attempt starts: the SDK consumes the
/// config when it does, and clearing keeps a config left by an attempt that never started from
/// being used by this one.
pub async fn set_peer_turn<R: Ratchet>(
    remote: &NodeRemote<R>,
    session_cid: u64,
    peer_cid: u64,
    turn: Option<&PeerTurnConfig>,
) -> Result<(), NetworkError> {
    let config = relay_config(turn, SystemTime::now());
    info!(target: "citadel", "[TURN] relay for {session_cid} -> {peer_cid}: {:?}", config.as_ref().map(|r| (r.policy, r.servers.len())));
    remote
        .send(NodeRequest::SetPeerTurnConfig(SetPeerTurnConfig {
            session_cid,
            peer_cid,
            config,
        }))
        .await
        .map(|_| ())
}

fn transport_rank(transport: TurnTransport) -> u8 {
    match transport {
        TurnTransport::Udp => 0,
        TurnTransport::Tcp => 1,
        TurnTransport::Tls => 2,
    }
}

pub fn path_report(path: P2pPath) -> P2pPathReport {
    match path {
        P2pPath::Direct => P2pPathReport::Direct,
        P2pPath::Turn => P2pPathReport::Turn,
        P2pPath::ServerRelay => P2pPathReport::ServerRelay,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use citadel_internal_service_types::IceServer;

    const NOW_SECS: u64 = 1_800_000_000;

    fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(NOW_SECS)
    }

    fn cloudflare_shaped(expires_at: u64) -> PeerTurnConfig {
        PeerTurnConfig {
            policy: WirePolicy::RelayOnly,
            ice_servers: vec![
                IceServer {
                    urls: vec!["stun:stun.cloudflare.com:3478".into()],
                    username: None,
                    credential: None,
                },
                IceServer {
                    urls: vec![
                        "turns:turn.cloudflare.com:443?transport=tcp".into(),
                        "turn:turn.cloudflare.com:80?transport=tcp".into(),
                        "turn:turn.cloudflare.com:3478?transport=udp".into(),
                    ],
                    username: Some("u".into()),
                    credential: Some("secret-credential".into()),
                },
            ],
            expires_at,
        }
    }

    #[test]
    fn turn_urls_only_udp_first_tls_last() {
        let cfg = relay_config(Some(&cloudflare_shaped(NOW_SECS + 300)), now()).unwrap();
        let order: Vec<_> = cfg
            .servers
            .iter()
            .map(|s| (s.url.port, s.url.transport))
            .collect();
        assert_eq!(
            order,
            [
                (3478, TurnTransport::Udp),
                (80, TurnTransport::Tcp),
                (443, TurnTransport::Tls)
            ]
        );
        assert_eq!(cfg.policy, TurnPolicy::RelayOnly);
        assert!(cfg.servers.iter().all(|s| !s.is_expired_at(now())));
        assert!(!format!("{cfg:?}").contains("secret-credential"));
    }

    #[test]
    fn absent_expired_or_empty_means_no_relay() {
        assert!(relay_config(None, now()).is_none());
        assert!(relay_config(Some(&cloudflare_shaped(NOW_SECS)), now()).is_none());
        assert!(relay_config(Some(&cloudflare_shaped(NOW_SECS - 1)), now()).is_none());
        let mut stun_only = cloudflare_shaped(NOW_SECS + 300);
        stun_only.ice_servers.truncate(1);
        assert!(relay_config(Some(&stun_only), now()).is_none());
        let mut no_credential = cloudflare_shaped(NOW_SECS + 300);
        no_credential.ice_servers[1].credential = None;
        assert!(relay_config(Some(&no_credential), now()).is_none());
    }

    #[test]
    fn policy_maps_through() {
        let mut fallback = cloudflare_shaped(NOW_SECS + 300);
        fallback.policy = WirePolicy::Fallback;
        let cfg = relay_config(Some(&fallback), now()).unwrap();
        assert_eq!(cfg.policy, TurnPolicy::Fallback);
    }
}
