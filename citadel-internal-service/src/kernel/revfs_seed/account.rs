//! Each account's RE-VFS roots, kept sealed in the account's own byte map beside
//! `_AGENT_SERVER_HOST` (server_host.rs). They are persisted and deleted with the account, and
//! no `LocalDB*` request can reach them. The current root is the seed the account's recovery
//! phrase encodes: stored when the registration issued the phrase, or when the phrase recovered
//! the account on this device. Nothing here draws one: an account with no stored root has no
//! RE-VFS key. A root never leaves the agent: only the key derived from it reaches the SDK, and
//! neither is logged.

use super::roots::Roots;
use super::seal::{self, SeedSealError, NONCE_LEN, SEED_LEN, VAULT_KEY_LEN};
use async_trait::async_trait;
use citadel_sdk::prelude::{NodeRemote, Ratchet, RevfsKey};

const KEY: &str = "_AGENT_REVFS_SEED";
const SUB_KEY: &str = "roots";
const SELF_PEER_CID: u64 = 0;

/// Why an account has no RE-VFS key on this device: none of its roots was ever stored here.
pub(crate) const NO_ROOT: &str = "This device holds no recovery-phrase root for the account, so \
     its files cannot be opened here: recover the account on this device with its recovery phrase";

/// Where an account's sealed roots are kept.
#[async_trait]
pub(crate) trait SeedRecords: Send + Sync {
    async fn load(&self, cid: u64) -> Result<Option<Vec<u8>>, String>;
    async fn store(&self, cid: u64, sealed: Vec<u8>) -> Result<(), String>;
}

#[async_trait]
impl<R: Ratchet> SeedRecords for NodeRemote<R> {
    async fn load(&self, cid: u64) -> Result<Option<Vec<u8>>, String> {
        self.account_manager()
            .get_persistence_handler()
            .get_byte_map_value(cid, SELF_PEER_CID, KEY, SUB_KEY)
            .await
            .map_err(|err| err.into_string())
    }

    async fn store(&self, cid: u64, sealed: Vec<u8>) -> Result<(), String> {
        self.account_manager()
            .get_persistence_handler()
            .store_byte_map_value(cid, SELF_PEER_CID, KEY, SUB_KEY, sealed)
            .await
            .map(|_previous| ())
            .map_err(|err| err.into_string())
    }
}

/// How a phrase's seed is taken as the account's root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Adopt {
    /// The phrase was just issued, or just recovered the account here: it is the root.
    Replace,
    /// This device already holds the account: what it stores is kept.
    IfAbsent,
}

pub(crate) async fn load(
    records: &dyn SeedRecords,
    vault_key: &[u8; VAULT_KEY_LEN],
    cid: u64,
) -> Result<Option<Roots>, String> {
    let Some(sealed) = records.load(cid).await? else {
        return Ok(None);
    };
    let plain = seal::open(&sealed, vault_key, cid).map_err(|err| match err {
        SeedSealError::Malformed => format!("the RE-VFS roots of account {cid} are malformed"),
        SeedSealError::DoesNotOpen => {
            format!("the RE-VFS roots of account {cid} do not open under this agent's vault key")
        }
    })?;
    Roots::decode(&plain)
        .map(Some)
        .ok_or_else(|| format!("the RE-VFS roots of account {cid} are malformed"))
}

pub(crate) async fn save(
    records: &dyn SeedRecords,
    vault_key: &[u8; VAULT_KEY_LEN],
    cid: u64,
    roots: &Roots,
) -> Result<(), String> {
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::fill(&mut nonce).map_err(|err| format!("no OS randomness for a nonce: {err}"))?;
    let sealed = seal::seal(&roots.encode(), vault_key, cid, nonce);
    records.store(cid, sealed).await
}

/// Takes `seed` (the phrase's) as the account's current root, as `how` says.
pub(crate) async fn adopt(
    records: &dyn SeedRecords,
    vault_key: &[u8; VAULT_KEY_LEN],
    cid: u64,
    seed: &[u8; SEED_LEN],
    how: Adopt,
) -> Result<(), String> {
    if how == Adopt::IfAbsent && load(records, vault_key, cid).await?.is_some() {
        return Ok(());
    }
    save(records, vault_key, cid, &Roots::new(seed)).await
}

/// The key of `root` for account `cid` on `domain` (its server, canonicalised).
pub(crate) fn key_of(root: &[u8; SEED_LEN], domain: &str, cid: u64) -> Result<RevfsKey, String> {
    RevfsKey::derive(root, domain, cid).map_err(|err| err.into_string())
}

/// The account's RE-VFS key on `domain`, from its stored current root; [`NO_ROOT`] without one.
pub(crate) async fn establish(
    records: &dyn SeedRecords,
    vault_key: &[u8; VAULT_KEY_LEN],
    cid: u64,
    domain: &str,
) -> Result<RevfsKey, String> {
    let roots = load(records, vault_key, cid)
        .await?
        .ok_or_else(|| NO_ROOT.to_string())?;
    key_of(&roots.current, domain, cid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::collections::HashMap;

    #[derive(Default)]
    pub(crate) struct Records(pub Mutex<HashMap<u64, Vec<u8>>>);

    #[async_trait]
    impl SeedRecords for Records {
        async fn load(&self, cid: u64) -> Result<Option<Vec<u8>>, String> {
            Ok(self.0.lock().get(&cid).cloned())
        }
        async fn store(&self, cid: u64, sealed: Vec<u8>) -> Result<(), String> {
            self.0.lock().insert(cid, sealed);
            Ok(())
        }
    }

    const VAULT: [u8; 32] = [4; 32];
    const PHRASE_SEED: [u8; 32] = [9; 32];

    #[tokio::test]
    async fn an_account_with_no_stored_root_has_no_key_and_none_is_drawn() {
        let records = Records::default();
        let refused = establish(&records, &VAULT, 7, "example.com").await;
        assert_eq!(refused.unwrap_err(), NO_ROOT);
        assert!(records.0.lock().is_empty(), "a root was drawn");
    }

    #[tokio::test]
    async fn the_key_is_the_phrase_seeds_and_it_is_stored_sealed() {
        let records = Records::default();
        adopt(&records, &VAULT, 7, &PHRASE_SEED, Adopt::Replace)
            .await
            .unwrap();
        let key = establish(&records, &VAULT, 7, "example.com").await.unwrap();
        let expected = RevfsKey::derive(&PHRASE_SEED, "example.com", 7).unwrap();
        assert_eq!(key.id(), expected.id());
        let sealed = records.0.lock().get(&7).cloned().unwrap();
        assert!(!sealed.windows(32).any(|w| w == PHRASE_SEED));
    }

    #[tokio::test]
    async fn a_held_account_keeps_its_root_and_a_recovered_one_takes_the_phrases() {
        let records = Records::default();
        adopt(&records, &VAULT, 7, &PHRASE_SEED, Adopt::Replace)
            .await
            .unwrap();
        adopt(&records, &VAULT, 7, &[1; 32], Adopt::IfAbsent)
            .await
            .unwrap();
        let roots = load(&records, &VAULT, 7).await.unwrap().unwrap();
        assert_eq!(
            *roots.current, PHRASE_SEED,
            "a held account's root was replaced"
        );
        adopt(&records, &VAULT, 8, &[1; 32], Adopt::IfAbsent)
            .await
            .unwrap();
        assert_eq!(
            *load(&records, &VAULT, 8).await.unwrap().unwrap().current,
            [1; 32]
        );
        adopt(&records, &VAULT, 7, &[1; 32], Adopt::Replace)
            .await
            .unwrap();
        assert_eq!(
            *load(&records, &VAULT, 7).await.unwrap().unwrap().current,
            [1; 32]
        );
    }

    #[tokio::test]
    async fn accounts_and_servers_get_unrelated_keys() {
        let records = Records::default();
        for cid in [7, 8] {
            adopt(&records, &VAULT, cid, &PHRASE_SEED, Adopt::Replace)
                .await
                .unwrap();
        }
        let a = establish(&records, &VAULT, 7, "example.com").await.unwrap();
        let b = establish(&records, &VAULT, 8, "example.com").await.unwrap();
        let other_server = establish(&records, &VAULT, 7, "example.org").await.unwrap();
        assert_ne!(a.id(), b.id());
        assert_ne!(a.id(), other_server.id());
    }

    #[tokio::test]
    async fn roots_under_another_vault_key_are_refused_not_replaced() {
        let records = Records::default();
        adopt(&records, &VAULT, 7, &PHRASE_SEED, Adopt::Replace)
            .await
            .unwrap();
        let sealed = records.0.lock().get(&7).cloned();
        assert!(establish(&records, &[5; 32], 7, "example.com")
            .await
            .is_err());
        assert!(adopt(&records, &[5; 32], 7, &[1; 32], Adopt::IfAbsent)
            .await
            .is_err());
        assert_eq!(records.0.lock().get(&7).cloned(), sealed);
    }
}
