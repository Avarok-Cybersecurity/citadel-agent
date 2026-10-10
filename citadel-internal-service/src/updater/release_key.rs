//! The public half of the ML-DSA-65 key that signs every agent release asset: the one copy the
//! agent trusts. Every update must carry `<asset>.mldsa.sig` by this key over the message
//! `citadel_release_signature` defines, as well as passing its sha256 and its attestation.
//!
//! The key is the text of `release_public_key.txt`, the line `release-sign keygen` in
//! citadel-workspace (`tools/release-sign`) printed; its private half is the repository secret
//! `CITADEL_RELEASE_MLDSA_KEY`. Were the file ever `citadel_release_signature::
//! PLACEHOLDER_PUBLIC_KEY` again, every update would be refused, never accepted, and the release
//! workflow would not publish; the test below fails first.

pub const RELEASE_PUBLIC_KEY: &str = include_str!("release_public_key.txt");

#[cfg(test)]
mod tests {
    use super::RELEASE_PUBLIC_KEY;
    use citadel_release_signature::parse_public_key;

    #[test]
    fn the_embedded_key_is_an_ml_dsa_65_key_not_the_placeholder() {
        if let Err(why) = parse_public_key(RELEASE_PUBLIC_KEY) {
            panic!("release_public_key.txt: {why}");
        }
    }
}
