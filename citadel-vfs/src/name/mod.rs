//! `Name` (`vfs-model.md` §6): the grammar (refused, never repaired) and the comparison key
//! `fold_key(name) = NFC(simple_case_fold(name))`, Unicode 17.0.0.

mod assigned_table;
mod fold_table;

use crate::limits::NAME_MAX_BYTES;
use unicode_normalization::UnicodeNormalization;

/// A valid entry name. The stored text keeps its case.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Name(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameError {
    Empty,
    TooLong,
    NotNfc,
    Unassigned,
    Slash,
    Control,
    Dot,
}

impl Name {
    pub fn new(text: &str) -> Result<Self, NameError> {
        check(text)?;
        Ok(Self(text.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Two siblings collide when these are equal.
    pub fn fold_key(&self) -> String {
        fold_key(&self.0)
    }
}

impl core::fmt::Display for Name {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The grammar of `vfs-model.md` §6, rules 1 to 5 in order.
pub fn check(text: &str) -> Result<(), NameError> {
    if text.is_empty() {
        return Err(NameError::Empty);
    }
    if text.len() > NAME_MAX_BYTES {
        return Err(NameError::TooLong);
    }
    if !unicode_normalization::is_nfc(text) {
        return Err(NameError::NotNfc);
    }
    if !text.chars().all(is_assigned) {
        return Err(NameError::Unassigned);
    }
    if text.contains('/') {
        return Err(NameError::Slash);
    }
    if text.chars().any(is_cc) {
        return Err(NameError::Control);
    }
    if text == "." || text == ".." {
        return Err(NameError::Dot);
    }
    Ok(())
}

/// `NFC(simple_case_fold(text))`.
pub fn fold_key(text: &str) -> String {
    text.chars().map(simple_case_fold).nfc().collect()
}

/// General category `Cc`: U+0000..=U+001F and U+007F..=U+009F.
pub fn is_cc(c: char) -> bool {
    matches!(u32::from(c), 0x00..=0x1F | 0x7F..=0x9F)
}

/// Assigned in Unicode 17.0.0 (`DerivedAge-17.0.0.txt` gives it an age).
pub fn is_assigned(c: char) -> bool {
    let cp = u32::from(c);
    let ranges = assigned_table::ASSIGNED_RANGES;
    // The last range starting at or before `cp`.
    let count = ranges.len() / 2;
    let (mut lo, mut hi) = (0usize, count);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if ranges[mid * 2] <= cp {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo > 0 && cp <= ranges[(lo - 1) * 2 + 1]
}

/// The simple case folding of `c` (statuses C and S), or `c` itself.
pub fn simple_case_fold(c: char) -> char {
    let cp = u32::from(c) as i64;
    let runs = fold_table::FOLD_RUNS;
    let count = runs.len() / 4;
    let (mut lo, mut hi) = (0usize, count);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if i64::from(runs[mid * 4]) <= cp {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo == 0 {
        return c;
    }
    let run = &runs[(lo - 1) * 4..lo * 4];
    let (start, n, stride, delta) = (
        i64::from(run[0]),
        i64::from(run[1]),
        i64::from(run[2]),
        i64::from(run[3]),
    );
    let offset = cp - start;
    if offset % stride != 0 || offset / stride >= n {
        return c;
    }
    u32::try_from(cp + delta)
        .ok()
        .and_then(char::from_u32)
        .unwrap_or(c)
}
