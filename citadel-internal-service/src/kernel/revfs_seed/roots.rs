//! An account's RE-VFS roots: the `current` one, which is the seed its recovery phrase encodes,
//! and the `retired` ones its objects may still be sealed under while a re-key after a phrase
//! regeneration is unfinished (rekey.rs). Pure: no I/O, no key storage.
//!
//! A root is never written in the clear: `encode` is what `seal.rs` seals.

use super::seal::SEED_LEN;
use citadel_sdk::prelude::RevfsKey;
use zeroize::Zeroizing;

pub(crate) type Root = Zeroizing<[u8; SEED_LEN]>;
/// `RevfsKey::id`: names a key on every group sealed under it and reveals nothing about it.
pub(crate) type KeyId = [u8; 8];

pub(crate) struct Roots {
    pub current: Root,
    pub retired: Vec<Root>,
}

impl std::fmt::Debug for Roots {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Roots(current, {} retired, redacted)",
            self.retired.len()
        )
    }
}

impl Roots {
    pub(crate) fn new(current: &[u8; SEED_LEN]) -> Self {
        Self {
            current: Zeroizing::new(*current),
            retired: Vec::new(),
        }
    }

    /// `current ‖ retired…`, each 32 bytes.
    pub(crate) fn encode(&self) -> Zeroizing<Vec<u8>> {
        let mut out = Zeroizing::new(Vec::with_capacity(SEED_LEN * (1 + self.retired.len())));
        out.extend_from_slice(self.current.as_slice());
        for root in &self.retired {
            out.extend_from_slice(root.as_slice());
        }
        out
    }

    /// `None` unless `bytes` is one or more whole roots.
    pub(crate) fn decode(bytes: &[u8]) -> Option<Self> {
        let (chunks, rest) = bytes.as_chunks::<SEED_LEN>();
        if chunks.is_empty() || !rest.is_empty() {
            return None;
        }
        let mut roots = chunks.iter().map(|chunk| Zeroizing::new(*chunk));
        let current = roots.next()?;
        Some(Self {
            current,
            retired: roots.collect(),
        })
    }

    /// `next` becomes the current root; the one it replaces is retired until no object needs it.
    pub(crate) fn rotate(&mut self, next: &[u8; SEED_LEN]) {
        if self.current.as_slice() == next.as_slice() {
            return;
        }
        let previous = std::mem::replace(&mut self.current, Zeroizing::new(*next));
        if !self.retired.iter().any(|root| **root == *previous) {
            self.retired.push(previous);
        }
    }

    /// Keeps the retired roots whose key on `domain` for `cid` is in `referenced`; whether any
    /// was dropped.
    pub(crate) fn retain_referenced(
        &mut self,
        referenced: &[KeyId],
        key_of: impl Fn(&Root) -> Result<RevfsKey, String>,
    ) -> Result<bool, String> {
        let before = self.retired.len();
        let mut kept = Vec::with_capacity(before);
        for root in self.retired.drain(..) {
            if referenced.contains(&key_of(&root)?.id()) {
                kept.push(root);
            }
        }
        self.retired = kept;
        Ok(self.retired.len() != before)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_of(root: &Root) -> Result<RevfsKey, String> {
        RevfsKey::derive(root, "acme.example", 7).map_err(|err| err.into_string())
    }

    #[test]
    fn roots_round_trip_and_a_partial_root_is_refused() {
        let mut roots = Roots::new(&[1; 32]);
        roots.rotate(&[2; 32]);
        let decoded = Roots::decode(&roots.encode()).unwrap();
        assert_eq!(*decoded.current, [2; 32]);
        assert_eq!(decoded.retired.len(), 1);
        assert_eq!(*decoded.retired[0], [1; 32]);
        assert!(Roots::decode(&[0; 33]).is_none());
        assert!(Roots::decode(&[]).is_none());
    }

    #[test]
    fn rotating_retires_the_current_root_once() {
        let mut roots = Roots::new(&[1; 32]);
        roots.rotate(&[1; 32]);
        assert!(roots.retired.is_empty(), "the same root is not a rotation");
        roots.rotate(&[2; 32]);
        roots.rotate(&[3; 32]);
        assert_eq!(*roots.current, [3; 32]);
        assert_eq!(roots.retired.len(), 2);
    }

    #[test]
    fn only_referenced_retired_roots_are_kept() {
        let mut roots = Roots::new(&[1; 32]);
        roots.rotate(&[2; 32]);
        roots.rotate(&[3; 32]);
        let still_used = key_of(&Zeroizing::new([2; 32])).unwrap().id();
        assert!(roots.retain_referenced(&[still_used], key_of).unwrap());
        assert_eq!(roots.retired.len(), 1);
        assert_eq!(*roots.retired[0], [2; 32]);
        assert!(!roots.retain_referenced(&[still_used], key_of).unwrap());
    }

    #[test]
    fn a_debug_string_holds_no_root() {
        let roots = Roots::new(&[0xAB; 32]);
        assert!(!format!("{roots:?}").contains("171"));
    }
}
