//! The node's key source answers for a session still being dialled: the server prompts a group
//! restore before the agent records the session, and a source without the key rejoins the group
//! without the state the server keeps for it.

use super::*;
use citadel_sdk::prelude::StackedRatchet;

fn key() -> RevfsKey {
    RevfsKey::derive(&[7; 32], "example.com", 9).expect("a key derives")
}

#[test]
fn a_session_being_dialled_has_its_key_before_it_is_recorded() {
    let sessions: Arc<RwLock<HashMap<u64, Connection<StackedRatchet>>>> = Default::default();
    let dialing = Dialing::default();
    let source = key_source(sessions, dialing.clone());
    assert!(source.revfs_key(9).is_none(), "no session, no key");

    let published = dialing.publish(9, key());
    assert_eq!(source.revfs_key(9).map(|k| k.id()), Some(key().id()));
    assert!(source.revfs_key(10).is_none(), "only the account dialled");

    drop(published);
    assert!(
        source.revfs_key(9).is_none(),
        "withdrawn once the dial is over"
    );
}
