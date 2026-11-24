//! `legacy_name` (`vfs-migration.md` §2).

use crate::limits::NAME_MAX_BYTES;
use crate::name::{is_cc, Name};
use unicode_normalization::UnicodeNormalization;

/// NFC; every `/` and `Cc` code point becomes `-`; cut to `NAME_MAX_BYTES` at a code-point
/// boundary; anything still not a name is `fallback`. `None` only if `fallback` is not a name.
pub fn legacy_name(raw: &str, fallback: &str) -> Option<Name> {
    let mut out = String::new();
    for c in raw
        .nfc()
        .map(|c| if c == '/' || is_cc(c) { '-' } else { c })
    {
        if out.len() + c.len_utf8() > NAME_MAX_BYTES {
            break;
        }
        out.push(c);
    }
    Name::new(&out).or_else(|_| Name::new(fallback)).ok()
}

/// A `RevfsNode` name: NFC only.
pub(super) fn node_name(raw: &str) -> Option<Name> {
    Name::new(&raw.nfc().collect::<String>()).ok()
}
