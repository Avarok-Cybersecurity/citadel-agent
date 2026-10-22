//! The grammar of a Citadel app's identity, defined once.
//!
//! An app is named by a reverse-DNS id (`com.acme.kanban`) and released as versions
//! (`1.4.0`). The publisher's signature binds both (`citadel-app-signature` signs only a
//! well-formed identity), the manifest is refused without them, the internal-HTTP wire carries
//! the id, and the kernel orders versions to refuse a downgrade. Every one of those reads the
//! grammar from here, so no two can disagree about what an id is.
//!
//! Pure: no dependencies, no I/O, the same on every target.

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

/// The longest app id, in bytes (an id is ASCII, so also in characters).
pub const MAX_APP_ID_CHARS: usize = 64;

/// Why an id or a version is not one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityRefusal {
    AppId(String),
    Version(String),
}

impl fmt::Display for IdentityRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AppId(why) => write!(f, "not an app id: {why}"),
            Self::Version(why) => write!(f, "not an app version: {why}"),
        }
    }
}

impl std::error::Error for IdentityRefusal {}

/// Accepts exactly `^[a-z0-9][a-z0-9-]*(\.[a-z0-9][a-z0-9-]*)+$` of at most
/// `MAX_APP_ID_CHARS`: two or more dot-separated labels of lowercase ASCII letters, digits and
/// hyphens, each starting with a letter or digit.
pub fn validate_app_id(id: &str) -> Result<(), IdentityRefusal> {
    let refuse = |why: &str| Err(IdentityRefusal::AppId(why.to_string()));
    if id.is_empty() {
        return refuse("it is empty");
    }
    if id.len() > MAX_APP_ID_CHARS {
        return refuse("it is longer than 64 characters");
    }
    let mut labels = 0;
    for label in id.split('.') {
        labels += 1;
        let mut bytes = label.bytes();
        match bytes.next() {
            None => return refuse("it has an empty label"),
            Some(first) if !(first.is_ascii_lowercase() || first.is_ascii_digit()) => {
                return refuse("a label starts with something other than a-z or 0-9")
            }
            Some(_) => {}
        }
        if !bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
            return refuse("a label holds something other than a-z, 0-9 and '-'");
        }
    }
    if labels < 2 {
        return refuse("it needs at least two dot-separated labels");
    }
    Ok(())
}

/// A release version: `MAJOR.MINOR.PATCH`, decimal, no leading zeros, each part a `u32`.
///
/// Pre-release and build suffixes are not accepted: an app version is compared to refuse
/// downgrades, and `1.0.0-rc.1 < 1.0.0` is an ordering nobody should have to get right in a
/// security check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AppVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl AppVersion {
    pub fn parse(text: &str) -> Result<Self, IdentityRefusal> {
        let mut parts = text.split('.');
        let mut next = |name: &str| -> Result<u32, IdentityRefusal> {
            let part = parts
                .next()
                .ok_or_else(|| IdentityRefusal::Version(format!("{text:?} has no {name} part")))?;
            parse_part(text, name, part)
        };
        let version = Self {
            major: next("major")?,
            minor: next("minor")?,
            patch: next("patch")?,
        };
        if parts.next().is_some() {
            return Err(IdentityRefusal::Version(format!(
                "{text:?} has more than three parts"
            )));
        }
        Ok(version)
    }
}

fn parse_part(text: &str, name: &str, part: &str) -> Result<u32, IdentityRefusal> {
    let refuse = |why: &str| {
        Err(IdentityRefusal::Version(format!(
            "{text:?}: the {name} {why}"
        )))
    };
    if part.is_empty() {
        return refuse("part is empty");
    }
    if !part.bytes().all(|b| b.is_ascii_digit()) {
        return refuse("part is not decimal digits");
    }
    if part.len() > 1 && part.starts_with('0') {
        return refuse("part has a leading zero");
    }
    match part.parse::<u32>() {
        Ok(value) => Ok(value),
        Err(_) => refuse("part does not fit in 32 bits"),
    }
}

impl Ord for AppVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch))
    }
}

impl PartialOrd for AppVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl FromStr for AppVersion {
    type Err = IdentityRefusal;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

impl fmt::Display for AppVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests;
