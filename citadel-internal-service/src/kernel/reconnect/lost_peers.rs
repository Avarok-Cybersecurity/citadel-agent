//! A dropped link takes the session's peer connections with it, and its windows are told so:
//! one `DisconnectNotification` per peer, the notice the SDK's own per-peer disconnect gives.
//!
//! The reconnect clears the peers before the SDK reports them one by one, so those later
//! reports find nothing and say nothing. Without this, a window went on showing its peers
//! connected, and the UI's auto-connect, which skips a connected peer, never redialled them
//! once the link was back.
use citadel_internal_service_types::{DisconnectNotification, InternalServiceResponse};
use std::collections::HashMap;

/// Removes every peer connection from `peers`, returning their CIDs in ascending order.
pub(crate) fn take<V>(peers: &mut HashMap<u64, V>) -> Vec<u64> {
    let mut lost: Vec<u64> = peers.drain().map(|(peer_cid, _)| peer_cid).collect();
    lost.sort_unstable();
    lost
}

/// What `cid`'s windows are told about the peers in `lost`, in that order.
pub(crate) fn notices(cid: u64, lost: &[u64]) -> Vec<InternalServiceResponse> {
    lost.iter()
        .map(|&peer_cid| {
            InternalServiceResponse::DisconnectNotification(DisconnectNotification {
                cid,
                peer_cid: Some(peer_cid),
                request_id: None,
                ended_locally: None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_lost_peer_is_named_to_the_session_that_lost_it() {
        let told = notices(7, &[11, 13]);
        let named: Vec<(u64, Option<u64>, bool)> = told
            .iter()
            .map(|response| match response {
                InternalServiceResponse::DisconnectNotification(n) => {
                    (n.cid, n.peer_cid, n.request_id.is_none())
                }
                other => panic!("not a disconnect notice: {other:?}"),
            })
            .collect();
        assert_eq!(named, vec![(7, Some(11), true), (7, Some(13), true)]);
    }

    #[test]
    fn taking_the_peers_empties_the_map() {
        let mut peers: HashMap<u64, &str> = HashMap::from([(13, "b"), (11, "a")]);
        assert_eq!(take(&mut peers), vec![11, 13]);
        assert!(peers.is_empty());
    }

    #[test]
    fn a_session_with_no_peers_is_told_nothing_more() {
        assert!(notices(7, &[]).is_empty());
    }
}
