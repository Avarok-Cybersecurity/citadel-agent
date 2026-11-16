//! The byte-level building blocks every format here shares: SHA3, HMAC-SHA3, the domain
//! separator, base32 and base64url, and a strict reader for length-prefixed blobs.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use sha3::{Digest, Sha3_256};

type HmacSha3_256 = Hmac<Sha3_256>;

/// `SHA3-256(part₀ ‖ part₁ ‖ …)`.
pub fn sha3_256(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha3_256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

/// `HMAC-SHA3-256(key, part₀ ‖ part₁ ‖ …)`.
pub fn hmac_sha3_256(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    // HMAC takes a key of any length; `new_from_slice` cannot fail for it.
    let mut mac = <HmacSha3_256 as Mac>::new_from_slice(key)
        .unwrap_or_else(|_| unreachable!("HMAC accepts every key length"));
    for part in parts {
        mac.update(part);
    }
    mac.finalize().into_bytes().into()
}

/// The one place a signed message gets its domain: `domain ‖ 0x00 ‖ body`. Every ML-DSA message
/// this crate signs or verifies is built here, so no signature by one key in one role is a
/// signature in another role, or in another protocol that signs the same fields.
pub fn domain_separated(domain: &[u8], body: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(domain.len() + 1 + body.len());
    message.extend_from_slice(domain);
    message.push(0);
    message.extend_from_slice(body);
    message
}

const BASE32_LOWER: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// RFC 4648 base32, lowercase, without padding.
pub fn base32_lower_nopad(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for byte in bytes {
        buffer = (buffer << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(BASE32_LOWER[((buffer >> bits) & 31) as usize]));
        }
    }
    if bits > 0 {
        out.push(char::from(
            BASE32_LOWER[((buffer << (5 - bits)) & 31) as usize],
        ));
    }
    out
}

/// RFC 4648 §5 base64url without padding.
pub fn base64url_encode(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Strict base64url without padding: padding, other alphabets and non-zero trailing bits are
/// refused, so every byte string has exactly one accepted text.
pub fn base64url_decode(text: &str) -> Result<Vec<u8>, String> {
    URL_SAFE_NO_PAD.decode(text).map_err(|e| e.to_string())
}

/// Appends `u16be(len s) ‖ s`, refusing a string longer than a u16 can count.
pub(crate) fn put_str16(out: &mut Vec<u8>, s: &str) -> Result<(), usize> {
    let len = u16::try_from(s.len()).map_err(|_| s.len())?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(s.as_bytes());
    Ok(())
}

/// Reads a blob front to back; every read is bounds-checked and `finish` refuses leftovers.
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    pub(crate) fn take(&mut self, n: usize, what: &str) -> Result<&'a [u8], String> {
        if self.bytes.len() < n {
            return Err(format!(
                "{what}: needs {n} bytes, {} remain",
                self.bytes.len()
            ));
        }
        let (head, tail) = self.bytes.split_at(n);
        self.bytes = tail;
        Ok(head)
    }

    pub(crate) fn array<const N: usize>(&mut self, what: &str) -> Result<[u8; N], String> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N, what)?);
        Ok(out)
    }

    pub(crate) fn u8(&mut self, what: &str) -> Result<u8, String> {
        Ok(self.array::<1>(what)?[0])
    }

    pub(crate) fn u64(&mut self, what: &str) -> Result<u64, String> {
        Ok(u64::from_be_bytes(self.array(what)?))
    }

    pub(crate) fn str16(&mut self, what: &str) -> Result<&'a str, String> {
        let len = u16::from_be_bytes(self.array(what)?);
        let bytes = self.take(usize::from(len), what)?;
        core::str::from_utf8(bytes).map_err(|_| format!("{what}: not UTF-8"))
    }

    pub(crate) fn finish(self) -> Result<(), String> {
        match self.bytes.len() {
            0 => Ok(()),
            n => Err(format!("{n} trailing bytes")),
        }
    }
}
