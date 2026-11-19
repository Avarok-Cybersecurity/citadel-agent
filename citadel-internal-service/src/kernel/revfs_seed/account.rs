//! Each account's RE-VFS root: `seed_rec`, drawn by this agent the first time the account signs
//! in here (SA-10 D2) and kept sealed in the account's own byte map, beside `_AGENT_SERVER_HOST`
//! (server_host.rs). It is persisted and deleted with the account, and no `LocalDB*` request can
//! reach it. The seed never leaves the agent: only the key derived from it reaches the SDK, and
//! neither is logged.

use super::seal::{self, SeedSealError, NONCE_LEN, SEED_LEN, VAULT_KEY_LEN};
use async_trait::async_trait;
use citadel_sdk::prelude::{NodeRemote, Ratchet, RevfsKey};
use zeroize::Zeroizing;

const KEY: &str = "_AGENT_REVFS_SEED";
const SUB_KEY: &str = "seed_rec";
const SELF_PEER_CID: u64 = 0;

/// Where an account's sealed seed is kept.
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

/// The account's RE-VFS key on `domain` (its server, canonicalised): from its stored seed, or
/// from a seed drawn now and stored sealed before the key is handed out.
pub(crate) async fn establish(
    records: &dyn SeedRecords,
    vault_key: &[u8; VAULT_KEY_LEN],
    cid: u64,
    domain: &str,
) -> Result<RevfsKey, String> {
    let seed = match records.load(cid).await? {
        Some(sealed) => seal::open(&sealed, vault_key, cid).map_err(|err| match err {
            SeedSealError::Malformed => format!("the RE-VFS seed of account {cid} is malformed"),
            SeedSealError::DoesNotOpen => format!(
                "the RE-VFS seed of account {cid} does not open under this agent's vault key"
            ),
        })?,
        None => {
            let mut seed = Zeroizing::new([0u8; SEED_LEN]);
            let mut nonce = [0u8; NONCE_LEN];
            getrandom::fill(seed.as_mut_slice())
                .and_then(|()| getrandom::fill(&mut nonce))
                .map_err(|err| format!("no OS randomness for a RE-VFS seed: {err}"))?;
            records
                .store(cid, seal::seal(&seed, vault_key, cid, nonce))
                .await?;
            seed
        }
    };
    RevfsKey::derive(&seed, domain, cid).map_err(|err| err.into_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Records(Mutex<HashMap<u64, Vec<u8>>>);

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

    #[tokio::test]
    async fn the_first_sign_in_seals_a_seed_and_later_ones_derive_the_same_key() {
        let records = Records::default();
        let first = establish(&records, &VAULT, 7, "example.com").await.unwrap();
        let sealed = records
            .0
            .lock()
            .get(&7)
            .cloned()
            .expect("the seed is stored");
        let again = establish(&records, &VAULT, 7, "example.com").await.unwrap();
        assert_eq!(first.id(), again.id());
        assert_eq!(
            records.0.lock().get(&7),
            Some(&sealed),
            "the seed is drawn once"
        );
    }

    #[tokio::test]
    async fn accounts_and_servers_get_unrelated_keys() {
        let records = Records::default();
        let a = establish(&records, &VAULT, 7, "example.com").await.unwrap();
        let b = establish(&records, &VAULT, 8, "example.com").await.unwrap();
        let other_server = establish(&records, &VAULT, 7, "example.org").await.unwrap();
        assert_ne!(a.id(), b.id());
        assert_ne!(a.id(), other_server.id());
    }

    #[tokio::test]
    async fn a_seed_under_another_vault_key_is_refused_not_replaced() {
        let records = Records::default();
        establish(&records, &VAULT, 7, "example.com").await.unwrap();
        let sealed = records.0.lock().get(&7).cloned();
        assert!(establish(&records, &[5; 32], 7, "example.com")
            .await
            .is_err());
        assert_eq!(
            records.0.lock().get(&7).cloned(),
            sealed,
            "a seed is never overwritten"
        );
    }
}
