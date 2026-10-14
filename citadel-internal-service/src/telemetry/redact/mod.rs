//! Pure redaction of one log message.
//!
//! Every rule reports the byte ranges it would replace, over the ORIGINAL text, and the
//! ranges are merged before anything is rewritten. Rewriting rule by rule would let a
//! later rule rescan an earlier placeholder, and resolving overlaps by discarding the
//! shorter range would leave the tail of a partly-matched secret in the output.
//!
//! A replaced value becomes `<kind:hash>`, where the hash is HMAC-SHA3-256 under the
//! batch salt, so one value maps to one placeholder within a batch and the salt makes
//! placeholders useless across batches and sessions.

mod rules;

use hmac::{Hmac, Mac};
use sha3::Sha3_256;

pub(crate) use rules::cid_tokens;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RedactionKind {
    Username,
    FullName,
    Secret,
    Email,
    Host,
    Path,
    File,
    Ip,
}

impl RedactionKind {
    fn label(self) -> &'static str {
        match self {
            Self::Username => "user",
            Self::FullName => "name",
            Self::Secret => "secret",
            Self::Email => "email",
            Self::Host => "host",
            Self::Path => "path",
            Self::File => "file",
            Self::Ip => "ip",
        }
    }

    /// Kinds whose values are case-insensitive, so `Alice` and `alice` share a hash.
    fn folds_case(self) -> bool {
        matches!(
            self,
            Self::Username | Self::FullName | Self::Email | Self::Host
        )
    }
}

/// A value the agent knows is identifying: a session's or a peer's username or full name,
/// a server password, a server host name. Matched exactly rather than guessed at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownValue {
    pub kind: RedactionKind,
    pub value: String,
}

/// The key for one batch's placeholders.
#[derive(Clone, PartialEq, Eq)]
pub struct Salt([u8; 32]);

impl Salt {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl std::fmt::Debug for Salt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Salt(..)")
    }
}

/// A range of the input one rule wants replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Hit {
    pub start: usize,
    pub end: usize,
    pub kind: RedactionKind,
}

/// The placeholder for `value` under `salt`.
pub fn placeholder(kind: RedactionKind, value: &str, salt: &Salt) -> String {
    let folded;
    let value = if kind.folds_case() {
        folded = value.to_ascii_lowercase();
        folded.as_str()
    } else {
        value
    };
    let mut mac =
        <Hmac<Sha3_256> as Mac>::new_from_slice(&salt.0).expect("HMAC accepts a key of any length");
    mac.update(kind.label().as_bytes());
    mac.update(&[0]);
    mac.update(value.as_bytes());
    let digest = mac.finalize().into_bytes();
    format!("<{}:{}>", kind.label(), hex::encode(&digest[..6]))
}

/// `input` with every identifying value replaced by its placeholder.
pub fn redact(input: &str, known: &[KnownValue], salt: &Salt) -> String {
    let mut hits = rules::known_value_hits(input, known);
    rules::pattern_hits(input, &mut hits);
    if hits.is_empty() {
        return input.to_owned();
    }
    let merged = merge(hits);
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0;
    for hit in merged {
        out.push_str(&input[cursor..hit.start]);
        out.push_str(&placeholder(hit.kind, &input[hit.start..hit.end], salt));
        cursor = hit.end;
    }
    out.push_str(&input[cursor..]);
    out
}

/// Overlapping or touching hits become one, so no part of any matched range survives.
/// The earliest-starting hit names the kind; known values win ties at the same start.
fn merge(mut hits: Vec<Hit>) -> Vec<Hit> {
    hits.sort_by_key(|hit| (hit.start, std::cmp::Reverse(hit.end)));
    let mut merged: Vec<Hit> = Vec::with_capacity(hits.len());
    for hit in hits {
        match merged.last_mut() {
            Some(last) if hit.start < last.end => last.end = last.end.max(hit.end),
            _ => merged.push(hit),
        }
    }
    merged
}

#[cfg(test)]
mod tests_engine;
#[cfg(test)]
mod tests_property;
#[cfg(test)]
mod tests_rules;
