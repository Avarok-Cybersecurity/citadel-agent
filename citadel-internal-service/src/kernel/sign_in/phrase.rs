//! The recovery phrase as a window types it, read into the SDK's `RecoveryPhrase` for the
//! account's server. Pure: the domain is derived by the caller (`kernel/recovery_domain.rs`).

use citadel_identity::CanonicalDomain;
use citadel_sdk::prelude::{RecoveryPhrase, RecoverySeed, SecBuffer};

/// The 24 words as typed (case and spacing do not matter), bound to `domain`. The refusal says
/// what is wrong with the words, never anything about an account.
pub(crate) fn read(typed: &SecBuffer, domain: &CanonicalDomain) -> Result<RecoveryPhrase, String> {
    let text = std::str::from_utf8(typed.as_ref())
        .map_err(|_| "That is not a recovery phrase".to_string())?;
    let seed = RecoverySeed::parse(text)
        .map_err(|err| format!("That is not a recovery phrase: {}", err.into_string()))?;
    RecoveryPhrase::new(seed, domain.as_str()).map_err(|err| err.into_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn domain() -> CanonicalDomain {
        citadel_identity::canonical_domain("acme.example").unwrap()
    }

    fn words(seed: [u8; 32]) -> String {
        RecoverySeed::from_bytes(seed).words().join(" ")
    }

    #[test]
    fn a_phrase_reads_back_to_its_seed_whatever_its_case_and_spacing() {
        let typed = words([9; 32]).to_uppercase().replace(' ', "   ");
        let phrase = read(&SecBuffer::from(typed.as_str()), &domain()).unwrap();
        assert_eq!(phrase.seed().as_bytes(), &[9; 32]);
        assert_eq!(phrase.canonical_domain(), "acme.example");
    }

    #[test]
    fn a_short_phrase_and_a_strange_word_are_refused_by_what_is_wrong() {
        let short = read(&SecBuffer::from("abandon ability"), &domain()).unwrap_err();
        assert!(short.contains("24 words"), "{short}");
        let mut typed: Vec<String> = words([1; 32]).split(' ').map(String::from).collect();
        typed[4] = "citadelx".into();
        let strange = read(&SecBuffer::from(typed.join(" ").as_str()), &domain()).unwrap_err();
        assert!(strange.contains("word 5"), "{strange}");
    }

    #[test]
    fn bytes_that_are_not_text_are_not_a_phrase() {
        let refused = read(&SecBuffer::from(vec![0xff, 0xfe]), &domain()).unwrap_err();
        assert_eq!(refused, "That is not a recovery phrase");
    }
}
