//! Proofs that let a window re-attach to a session without the password.
//!
//! A password attach (`AttachSession` with `AttachProof::Password`) mints one,
//! and the browser keeps it sealed. Presenting it later proves the same thing
//! the password proved -- that this browser already showed it knows the
//! password for THIS session -- so a reload does not prompt again.
//!
//! In memory only, on the session: logout, deregister, a server give-up or an
//! agent restart ends them all. Random bytes from the OS, compared with the
//! same constant-time comparison as the credential fingerprint. Capped, oldest
//! out, so a caller that keeps attaching cannot grow the session without bound.

use crate::kernel::credential_fingerprint;
use std::collections::VecDeque;

/// Enough for a handful of browsers and profiles per account.
pub(crate) const MAX_TOKENS: usize = 8;
const TOKEN_BYTES: usize = 32;

#[derive(Default)]
pub(crate) struct AttachTokens {
    issued: VecDeque<Vec<u8>>,
}

impl AttachTokens {
    /// A fresh token, or `None` if the OS has no randomness to give -- which
    /// the caller must treat as a failure, never as an empty token.
    pub(crate) fn mint(&mut self) -> Option<Vec<u8>> {
        let mut token = vec![0u8; TOKEN_BYTES];
        getrandom::fill(&mut token).ok()?;
        if self.issued.len() == MAX_TOKENS {
            self.issued.pop_front();
        }
        self.issued.push_back(token.clone());
        Some(token)
    }

    /// Whether `presented` is one this session issued.
    pub(crate) fn admits(&self, presented: &[u8]) -> bool {
        let presented = presented.to_vec();
        // Every entry is compared, so the time taken does not say which matched.
        self.issued.iter().fold(false, |found, issued| {
            credential_fingerprint::matches(Some(issued), Some(&presented)) | found
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minted_token_is_admitted() {
        let mut tokens = AttachTokens::default();
        let token = tokens.mint().expect("randomness");
        assert_eq!(token.len(), TOKEN_BYTES);
        assert!(tokens.admits(&token));
    }

    #[test]
    fn a_token_this_session_never_issued_is_refused() {
        let mut tokens = AttachTokens::default();
        let token = tokens.mint().expect("randomness");
        let mut forged = token.clone();
        forged[0] ^= 1;
        assert!(!tokens.admits(&forged));
        assert!(
            !AttachTokens::default().admits(&token),
            "another session's token"
        );
    }

    #[test]
    fn an_empty_presentation_is_refused() {
        let mut tokens = AttachTokens::default();
        tokens.mint();
        assert!(!tokens.admits(&[]));
    }

    #[test]
    fn two_tokens_differ() {
        let mut tokens = AttachTokens::default();
        assert_ne!(tokens.mint(), tokens.mint());
    }

    #[test]
    fn the_oldest_token_is_evicted_past_the_cap() {
        let mut tokens = AttachTokens::default();
        let first = tokens.mint().expect("randomness");
        for _ in 0..MAX_TOKENS {
            tokens.mint();
        }
        assert!(!tokens.admits(&first));
        assert_eq!(tokens.issued.len(), MAX_TOKENS);
    }
}
