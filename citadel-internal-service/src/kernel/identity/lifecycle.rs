//! Creating, unlocking, exporting and importing the identity against its store. Each returns
//! the outcome for the UI or the refusal; the request handlers only frame them.

use super::record::IdentityRecord;
use super::seal::{seal, unseal, SealError, SEAL_PARAMS};
use super::IdentityHost;
use citadel_identity::{phrase_decode, phrase_encode, Identity, SEED_LEN};
use citadel_internal_service_types::{IdentityOutcome, IdentityRefusal, IdentityState};
use zeroize::Zeroizing;

pub(crate) type Refused = (IdentityRefusal, String);

fn storage(err: String) -> Refused {
    (IdentityRefusal::Storage, err)
}

fn random<const N: usize>() -> Result<[u8; N], Refused> {
    let mut out = [0u8; N];
    getrandom::fill(&mut out).map_err(|err| storage(format!("no OS randomness: {err}")))?;
    Ok(out)
}

fn now_s() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Argon2id is seconds of CPU: off the async workers.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Refused> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|err| storage(err.to_string()))
}

impl IdentityHost {
    async fn record(&self) -> Result<Option<IdentityRecord>, Refused> {
        match self.store.load().await.map_err(storage)? {
            Some(bytes) => IdentityRecord::decode(&bytes).map(Some).map_err(storage),
            None => Ok(None),
        }
    }

    pub(crate) async fn status(&self) -> Result<IdentityOutcome, Refused> {
        let state = match (self.record().await?, self.unlocked.current()) {
            (None, _) => IdentityState::None,
            (Some(r), Some(_)) => IdentityState::Unlocked { id_fp: r.id_fp },
            (Some(r), None) => IdentityState::Locked { id_fp: r.id_fp },
        };
        Ok(IdentityOutcome::Status { state })
    }

    /// Seals `seed` under `password`, stores it, and holds the identity unlocked.
    async fn adopt(
        &self,
        seed: Zeroizing<[u8; SEED_LEN]>,
        password: Zeroizing<Vec<u8>>,
    ) -> Result<(String, String), Refused> {
        if self.record().await?.is_some() {
            return Err((
                IdentityRefusal::AlreadyExists,
                "this agent already holds an identity".into(),
            ));
        }
        let (salt, nonce) = (random::<32>()?, random::<12>()?);
        let (identity, sealed) = blocking(move || {
            let identity = Identity::from_seed(&seed);
            let sealed = seal(
                &seed,
                &password,
                &identity.id_fp(),
                salt,
                nonce,
                SEAL_PARAMS,
            );
            (identity, sealed)
        })
        .await?;
        let sealed = sealed.map_err(|err| storage(format!("sealing the seed: {err:?}")))?;
        let record = IdentityRecord::new(identity.public_key().clone(), now_s(), sealed);
        self.store.save(record.encode()).await.map_err(storage)?;
        let identity = self.unlocked.hold(identity);
        Ok((identity.public_key().to_base64url(), identity.id_fp()))
    }

    pub(crate) async fn create(
        &self,
        password: Zeroizing<Vec<u8>>,
    ) -> Result<IdentityOutcome, Refused> {
        let seed = Zeroizing::new(random::<SEED_LEN>()?);
        let (pk, id_fp) = self.adopt(seed, password).await?;
        Ok(IdentityOutcome::Created { pk, id_fp })
    }

    pub(crate) async fn import(
        &self,
        phrase: Zeroizing<Vec<u8>>,
        password: Zeroizing<Vec<u8>>,
    ) -> Result<IdentityOutcome, Refused> {
        let bad = |why: String| (IdentityRefusal::BadPhrase, why);
        let text =
            std::str::from_utf8(&phrase).map_err(|_| bad("the phrase is not text".into()))?;
        let words: Vec<&str> = text.split_whitespace().collect();
        let seed = Zeroizing::new(phrase_decode(&words).map_err(|err| bad(err.to_string()))?);
        let (pk, id_fp) = self.adopt(seed, password).await?;
        Ok(IdentityOutcome::Imported { pk, id_fp })
    }

    /// The seed, opened with `password`.
    pub(super) async fn open(
        &self,
        password: Zeroizing<Vec<u8>>,
    ) -> Result<Zeroizing<[u8; SEED_LEN]>, Refused> {
        let record = self.record().await?.ok_or((
            IdentityRefusal::NoIdentity,
            "this agent holds no identity".to_string(),
        ))?;
        let opened = blocking(move || unseal(&record.sealed, &password, &record.id_fp)).await?;
        opened.map_err(|err| match err {
            SealError::WrongPassword => (
                IdentityRefusal::WrongPassword,
                "the unlock password is wrong".to_string(),
            ),
            other => storage(format!("the sealed identity: {other:?}")),
        })
    }

    pub(crate) async fn unlock(
        &self,
        password: Zeroizing<Vec<u8>>,
    ) -> Result<IdentityOutcome, Refused> {
        let seed = self.open(password).await?;
        let identity = self.unlocked.hold(Identity::from_seed(&seed));
        Ok(IdentityOutcome::Unlocked {
            id_fp: identity.id_fp(),
        })
    }

    pub(crate) async fn export(
        &self,
        password: Zeroizing<Vec<u8>>,
    ) -> Result<IdentityOutcome, Refused> {
        let seed = self.open(password).await?;
        let phrase = phrase_encode(&seed).iter().map(|w| w.to_string()).collect();
        Ok(IdentityOutcome::Exported { phrase })
    }
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;
