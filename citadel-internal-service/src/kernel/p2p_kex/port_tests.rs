use super::*;
use citadel_identity::Identity;

/// The identity crate signs under the domain the SDK verifies under. That the signatures
/// themselves agree is proved end to end (tests/two_identities_sign_their_p2p_exchanges.rs):
/// a signature the SDK did not accept would refuse every connection there.
#[test]
fn the_identity_signer_signs_under_the_sdks_transcript_domain() {
    assert_eq!(
        citadel_identity::P2P_KEX_DOMAIN,
        citadel_sdk::prelude::KEX_TRANSCRIPT_DOMAIN
    );
    let signer = Identity::from_seed(&[5; 32]).p2p_kex_signer();
    assert_ne!(signer.sign(b"one body"), signer.sign(b"another body"));
}

#[test]
fn a_signed_exchange_is_marked_with_the_peers_fingerprint_and_anything_else_is_not() {
    let pk = Identity::from_seed(&[6; 32]).pk().to_vec();
    let signed = PeerKexIdentity::Signed {
        identity_key: pk.clone(),
    };
    assert_eq!(
        mark(Some(&signed)),
        PeerIdentityMark::Verified {
            fingerprint: grouped_fingerprint_of(&pk).unwrap()
        }
    );
    assert_eq!(
        mark(Some(&PeerKexIdentity::Unsigned)),
        PeerIdentityMark::Unverified
    );
    assert_eq!(mark(None), PeerIdentityMark::Unverified);
}
