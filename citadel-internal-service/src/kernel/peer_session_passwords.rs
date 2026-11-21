//! The peer session password a session's windows last gave for each peer, on a
//! PeerRegister, PeerConnect or PeerConnectAccept, so the agent's own answer to that
//! peer's offer (kernel/inbound_connect) carries the password a window's would.
//! Memory only, and gone with the session; a request without one clears it.

use citadel_sdk::prelude::PreSharedKey;
use std::collections::HashMap;

#[derive(Default)]
pub(crate) struct PeerSessionPasswords(HashMap<u64, PreSharedKey>);

impl PeerSessionPasswords {
    pub(crate) fn remember(&mut self, peer_cid: u64, password: Option<&PreSharedKey>) {
        match password {
            Some(password) => {
                self.0.insert(peer_cid, password.clone());
            }
            None => {
                self.0.remove(&peer_cid);
            }
        }
    }

    pub(crate) fn for_peer(&self, peer_cid: u64) -> Option<PreSharedKey> {
        self.0.get(&peer_cid).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_password_given_for_a_peer_is_the_one_kept() {
        let mut passwords = PeerSessionPasswords::default();
        passwords.remember(7, Some(&PreSharedKey::from("one".as_bytes())));
        passwords.remember(7, Some(&PreSharedKey::from("two".as_bytes())));
        assert_eq!(
            passwords.for_peer(7).map(|p| format!("{p:?}")),
            Some(format!("{:?}", PreSharedKey::from("two".as_bytes())))
        );
        assert!(passwords.for_peer(8).is_none());
        passwords.remember(7, None);
        assert!(passwords.for_peer(7).is_none());
    }
}
