//! The hash and framing primitives every derivation here shares.

use sha3::{Digest, Sha3_256};

/// `SHA3-256(part₀ ‖ part₁ ‖ …)`: ids, op hashes and chains (`vfs-model.md` header).
pub fn sha3_256(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha3_256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

/// A string longer than `lp` can frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooLong;

/// `lp(s) = u16be(len(s)) ‖ s` (`citadel-identity`'s `put_str16`).
pub fn lp(out: &mut Vec<u8>, s: &str) -> Result<(), TooLong> {
    let len = u16::try_from(s.len()).map_err(|_| TooLong)?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(s.as_bytes());
    Ok(())
}

/// Lowercase hex.
pub fn to_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from(DIGITS[usize::from(b >> 4)]));
        out.push(char::from(DIGITS[usize::from(b & 15)]));
    }
    out
}

/// Exactly `N` bytes from lowercase hex; anything else is refused.
pub fn from_hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let raw = text.as_bytes();
    if raw.len() != N * 2 {
        return None;
    }
    let nibble = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    let mut out = [0u8; N];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = (nibble(raw[2 * i])? << 4) | nibble(raw[2 * i + 1])?;
    }
    Some(out)
}
