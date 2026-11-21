//! `IdentitySignChallenge` (`sign-in-with-citadel.md` §1a, §8; `identity-api.md` §7a): the
//! identity key signs the issuer's attempt for the sign-in page. The signed `origin` is the
//! one the connection's handshake carried (kernel/control_origin.rs), which the caller passes
//! in; nothing in the request names it. The unlock password is re-proven for every signature
//! against the sealed seed, exactly as `IdentityExport` re-proves it, and the lock state is
//! left as it was.

use super::lifecycle::Refused;
use super::sign_gate::Refusal;
use super::IdentityHost;
use citadel_identity::{base64url_encode, Identity, MAX_PROOF_FIELD_BYTES, NONCE_LEN};
use citadel_internal_service_types::{IdentityOutcome, IdentityRefusal};
use uuid::Uuid;
use zeroize::Zeroizing;

/// What the page asks to be signed: everything but the origin.
pub(crate) struct SignRequest {
    pub(crate) issuer: String,
    pub(crate) aud: String,
    pub(crate) nonce: Vec<u8>,
    pub(crate) challenge: Vec<u8>,
}

fn bad(why: String) -> Refused {
    (IdentityRefusal::BadField, why)
}

fn exactly_32(name: &str, bytes: &[u8]) -> Result<[u8; NONCE_LEN], Refused> {
    bytes.try_into().map_err(|_| {
        bad(format!(
            "{name} must be {NONCE_LEN} bytes, not {}",
            bytes.len()
        ))
    })
}

fn checked(request: &SignRequest) -> Result<([u8; NONCE_LEN], [u8; NONCE_LEN]), Refused> {
    for (name, value) in [("issuer", &request.issuer), ("aud", &request.aud)] {
        if value.is_empty() || value.len() > MAX_PROOF_FIELD_BYTES {
            return Err(bad(format!(
                "{name} must be 1 to {MAX_PROOF_FIELD_BYTES} bytes"
            )));
        }
    }
    Ok((
        exactly_32("nonce", &request.nonce)?,
        exactly_32("challenge", &request.challenge)?,
    ))
}

fn rate_limited(refusal: Refusal) -> Refused {
    let message = match refusal {
        Refusal::TooSoon { wait } => format!("one signature at a time: wait {wait:?}"),
        Refusal::LockedOut { wait } => {
            format!("too many wrong passwords: signing is refused for {wait:?}")
        }
    };
    (IdentityRefusal::RateLimited, message)
}

impl IdentityHost {
    /// The proof for `request` on connection `conn`, whose handshake carried `origin`.
    pub(crate) async fn sign_challenge(
        &self,
        conn: Uuid,
        origin: Option<String>,
        request: SignRequest,
        password: Zeroizing<Vec<u8>>,
    ) -> Result<IdentityOutcome, Refused> {
        let (nonce, challenge) = checked(&request)?;
        let origin = origin.ok_or((
            IdentityRefusal::NoBrowserOrigin,
            "this connection carried no browser Origin: only a sign-in page can ask for a proof"
                .to_string(),
        ))?;
        self.sign_gate
            .lock()
            .admit(conn, self.clock.now())
            .map_err(rate_limited)?;
        let seed = match self.open(password).await {
            Err((IdentityRefusal::WrongPassword, message)) => {
                self.sign_gate.lock().wrong_password(self.clock.now());
                return Err((IdentityRefusal::WrongPassword, message));
            }
            opened => opened?,
        };
        self.sign_gate.lock().right_password();
        let identity = Identity::from_seed(&seed);
        let blob = identity
            .sign_proof(&request.issuer, &request.aud, &origin, &nonce, &challenge)
            .map_err(|err| bad(err.to_string()))?;
        Ok(IdentityOutcome::SignedChallenge {
            proof: base64url_encode(&blob),
        })
    }
}

#[cfg(test)]
#[path = "sign_challenge_tests.rs"]
mod tests;
