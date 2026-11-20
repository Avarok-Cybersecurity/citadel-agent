//! The portable Citadel identity (`ACCOUNT-IDENTITY-V2-PORTABLE.md` rev b; API: batch-1116
//! master plan §5.1). One crate holds every derivation and every verifier, compiled natively
//! for the agent and the kernel and to wasm32-unknown-unknown for the browser: names
//! (`handle@canonical-domain`), the identity key and its fingerprint, per-server passwords,
//! bindings, the recovery phrase, `/.well-known/citadel`, the server challenge, and the
//! "Sign in with Citadel" identity proof and assertion.
//!
//! Pure: no I/O, no randomness (seeds and nonces are the caller's), no clock (`now_s` is the
//! caller's). Post-quantum only: ML-DSA-65, SHA3-256, HMAC-SHA3-256.
//!
//! Every signed message is `domain ‖ 0x00 ‖ body`, built in one place (`codec::domain_separated`)
//! with a domain of its own, so no signature in one role is a signature in another, and no
//! signature this key makes in another protocol (an app signature, an agent release) is one here.

mod assertion;
mod binding;
mod cbor;
mod codec;
mod domain;
mod identity;
mod keys;
mod names;
mod p2p_kex;
mod phrase;
mod pin;
mod proof;
mod server;
mod well_known;
mod well_known_error;
mod wordlist;

/// The kernel's username cap (u8 maximum). SSOT here; the kernel's test mirrors it.
pub const MAX_USERNAME_BYTES: usize = 255;
pub const MIN_HANDLE_BYTES: usize = 3;
pub const MAX_HANDLE_BYTES: usize = 64;
/// 64 + 1 + 190 = 255.
pub const MAX_DOMAIN_BYTES: usize = 190;
/// Seconds an agent waits for the server's answer to its `ServerIdentityChallenge` before it
/// refuses the sign-in. A protocol value, not an operator's choice.
pub const SERVER_CHALLENGE_TIMEOUT_S: u64 = 10;
/// Seconds the whole `/.well-known/citadel` exchange may take. A protocol value, likewise.
pub const WELL_KNOWN_FETCH_TIMEOUT_S: u64 = 10;
/// The agent's `IdentitySignChallenge` rate (`sign-in-with-citadel.md` §8), protocol values like
/// the two above: at most one request per this many seconds on one connection;
pub const SIGN_MIN_INTERVAL_S: u64 = 2;
/// this many wrong passwords in a row start a lockout;
pub const SIGN_WRONG_PASSWORD_LIMIT: u32 = 5;
/// which refuses every request for this many seconds.
pub const SIGN_LOCKOUT_S: u64 = 60;

pub use assertion::{
    assertion_message, open_assertion, verify_assertion, AssertionError, SignInAssertion,
    ASSERTION_DOMAIN, ASSERTION_VERSION, MAX_ASSERTION_FIELD_BYTES,
};
pub use binding::{
    binding_from_base64url, binding_message, verify_binding, BindingBlob, BindingError, Verified,
    BINDING_DOMAIN, BINDING_FIXED_LEN, BINDING_VERSION, MAX_BINDING_BASE64_LEN,
    MAX_BINDING_FUTURE_SKEW_S,
};
pub use codec::{base32_lower_nopad, base64url_decode, base64url_encode};
pub use domain::{canonical_domain, CanonicalDomain, DomainError};
pub use identity::{
    Identity, Password, PASSWORD_LEN, PASSWORD_SUBKEY_LABEL, SERVER_PASSWORD_DOMAIN,
    SIGN_SUBKEY_LABEL,
};
pub use keys::{
    fingerprint, fingerprint_grouped, MlDsa65Pk, SignatureError, FINGERPRINT_BYTES,
    FINGERPRINT_DOMAIN, PUBLIC_KEY_LEN, SEED_LEN, SIGNATURE_LEN,
};
pub use names::{
    display_handle, make_username, parse_username, validate_handle, NameError, Username,
};
pub use p2p_kex::{P2pKexSigner, P2P_KEX_DOMAIN};
pub use phrase::{
    match_phrase, phrase_decode, phrase_encode, PhraseError, MATCH_PHRASE_DOMAIN, PHRASE_WORDS,
};
pub use pin::check_against_pin;
pub use proof::{
    proof_message, verify_proof, ProofError, SignInProof, MAX_PROOF_FIELD_BYTES, PROOF_DOMAIN,
    PROOF_FIXED_LEN, PROOF_VERSION,
};
pub use server::{
    challenge_message, verify_server_challenge, ChallengeError, ServerKey, CHALLENGE_DOMAIN,
    NONCE_LEN, SERVER_KEY_LABEL,
};
pub use well_known::{
    parse_well_known, well_known_message, WellKnownCitadel, WellKnownError, SERVER_KEY_ALG,
    WELL_KNOWN_DOMAIN, WELL_KNOWN_VERSION,
};
