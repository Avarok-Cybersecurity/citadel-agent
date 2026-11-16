//! The challenge judgement: only the pinned server's signature over this nonce, this CID and
//! the pinned domain passes.
//!
//! Control: return `Ok(offered)` before the key comparison in `judge` → the key-swapped answer
//! is accepted and `a_key_swapped_server_is_refused_with_both_fingerprints` goes red.

use super::*;
use citadel_identity::{canonical_domain, ServerKey};

const CID: u64 = 0x0123_4567_89ab_cdef;
const NONCE: [u8; 32] = [0x5a; 32];

fn server() -> ServerKey {
    ServerKey::from_seed(&[11; 32])
}

fn pin(verified: bool) -> Pin {
    let key = server();
    Pin {
        handle: "alice".into(),
        domain: canonical_domain("acme.example").unwrap(),
        server_fp: fingerprint(key.public_key()),
        server_pk: verified.then(|| key.public_key().clone()),
        issued_at_s: None,
        id_fp: "3foriftckvpoy2rh".into(),
    }
}

fn answer(key: &ServerKey, domain: &str, cid: u64, nonce: &[u8; 32]) -> ServerIdentityAnswer {
    let signed_as = canonical_domain(domain).unwrap();
    ServerIdentityAnswer {
        domain: domain.into(),
        pk: key.public_key().as_bytes().to_vec(),
        sig: key.sign_challenge(&signed_as, cid, nonce),
    }
}

#[test]
fn the_pinned_server_passes_verified_or_by_fingerprint() {
    let good = answer(&server(), "acme.example", CID, &NONCE);
    for verified in [true, false] {
        let key = judge(&pin(verified), CID, &NONCE, &good).unwrap();
        assert_eq!(key, *server().public_key());
    }
}

#[test]
fn a_key_swapped_server_is_refused_with_both_fingerprints() {
    let swapped = ServerKey::from_seed(&[13; 32]);
    let forged = answer(&swapped, "acme.example", CID, &NONCE);
    for verified in [true, false] {
        let refused = judge(&pin(verified), CID, &NONCE, &forged).unwrap_err();
        assert_eq!(refused.pinned_fp, fingerprint(server().public_key()));
        assert_eq!(refused.offered_fp, fingerprint(swapped.public_key()));
    }
}

#[test]
fn a_replay_for_another_cid_nonce_or_domain_is_refused() {
    let p = pin(true);
    let other_cid = answer(&server(), "acme.example", CID + 1, &NONCE);
    assert!(judge(&p, CID, &NONCE, &other_cid).is_err());
    let other_nonce = answer(&server(), "acme.example", CID, &[0; 32]);
    assert!(judge(&p, CID, &NONCE, &other_nonce).is_err());
    let other_domain = answer(&server(), "acme.work.avarok.net", CID, &NONCE);
    assert!(judge(&p, CID, &NONCE, &other_domain).is_err());
    let mut no_key = answer(&server(), "acme.example", CID, &NONCE);
    no_key.pk.truncate(10);
    assert_eq!(judge(&p, CID, &NONCE, &no_key).unwrap_err().offered_fp, "");
}

struct FirstByte;

impl ServerIdentityCodec for FirstByte {
    fn challenge(&self, nonce: &[u8; NONCE_LEN]) -> Result<Vec<u8>, String> {
        Ok(nonce.to_vec())
    }

    fn answer(&self, frame: &[u8]) -> Option<ServerIdentityAnswer> {
        (frame.first() == Some(&1)).then(|| ServerIdentityAnswer {
            domain: "acme.example".into(),
            pk: Vec::new(),
            sig: Vec::new(),
        })
    }
}

#[test]
fn only_an_awaited_answer_is_kept_from_the_windows() {
    let waiters = ChallengeWaiters::new(std::sync::Arc::new(FirstByte));
    assert!(
        waiters.on_server_frame(CID, &[0, 9]),
        "not an answer: the windows'"
    );
    assert!(
        waiters.on_server_frame(CID, &[1]),
        "nobody waits: a window asked"
    );
    let mut rx = waiters.expect(CID);
    assert!(waiters.on_server_frame(CID + 1, &[1]), "another session's");
    assert!(
        !waiters.on_server_frame(CID, &[1]),
        "the agent's own answer stops here"
    );
    assert!(rx.try_recv().is_ok());
}
