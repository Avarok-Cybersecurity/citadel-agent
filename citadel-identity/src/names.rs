//! Names: the person's handle and the Citadel username `handle@domain` the agent, the SDK,
//! the server and peers see (identity doc §2, §3.1 rev b).

use crate::domain::{canonical_domain, CanonicalDomain, DomainError};
use crate::{MAX_HANDLE_BYTES, MAX_USERNAME_BYTES, MIN_HANDLE_BYTES};
use std::fmt;

/// A Citadel username: a V2 `handle@domain`, or a legacy name with no `@` (dots allowed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Username<'a> {
    Legacy(&'a str),
    V2 { handle: &'a str, domain: &'a str },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameError {
    Empty,
    TooLong {
        len: usize,
    },
    HandleLength {
        len: usize,
    },
    HandleChar(char),
    /// A leading or trailing `.`, or `..`.
    HandleDots,
    /// Two or more `@`: neither a V2 name nor a legacy one.
    TooManyAt,
    /// The part after `@` is not a canonical domain.
    Domain(DomainError),
    NotCanonical(String),
}

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "the name is empty"),
            Self::TooLong { len } => write!(
                f,
                "the name is {len} bytes, over the {MAX_USERNAME_BYTES} allowed"
            ),
            Self::HandleLength { len } => write!(
                f,
                "a handle is {MIN_HANDLE_BYTES} to {MAX_HANDLE_BYTES} bytes, not {len}"
            ),
            Self::HandleChar(c) => write!(f, "{c:?} cannot be part of a handle"),
            Self::HandleDots => write!(f, "a handle cannot start or end with '.' or hold \"..\""),
            Self::TooManyAt => write!(f, "a name holds at most one '@'"),
            Self::Domain(why) => write!(f, "{why}"),
            Self::NotCanonical(d) => write!(f, "{d:?} is not a canonical server name"),
        }
    }
}

impl std::error::Error for NameError {}

/// `3..=64` bytes of `[a-z0-9._-]`, no leading or trailing `.`, no `..`.
pub fn validate_handle(handle: &str) -> Result<(), NameError> {
    if !(MIN_HANDLE_BYTES..=MAX_HANDLE_BYTES).contains(&handle.len()) {
        return Err(NameError::HandleLength { len: handle.len() });
    }
    if let Some(c) = handle
        .chars()
        .find(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(*c)))
    {
        return Err(NameError::HandleChar(c));
    }
    if handle.starts_with('.') || handle.ends_with('.') || handle.contains("..") {
        return Err(NameError::HandleDots);
    }
    Ok(())
}

/// Exactly one `@` → V2 (handle validated, domain canonical); none → legacy; more → refused.
pub fn parse_username(username: &str) -> Result<Username<'_>, NameError> {
    if username.is_empty() {
        return Err(NameError::Empty);
    }
    if username.len() > MAX_USERNAME_BYTES {
        return Err(NameError::TooLong {
            len: username.len(),
        });
    }
    let Some((handle, domain)) = username.split_once('@') else {
        return Ok(Username::Legacy(username));
    };
    if domain.contains('@') {
        return Err(NameError::TooManyAt);
    }
    validate_handle(handle)?;
    let canonical = canonical_domain(domain).map_err(NameError::Domain)?;
    if canonical.as_str() != domain {
        return Err(NameError::NotCanonical(domain.to_string()));
    }
    Ok(Username::V2 { handle, domain })
}

/// `handle@domain`, validated as `parse_username` would.
pub fn make_username(handle: &str, domain: &CanonicalDomain) -> Result<String, NameError> {
    validate_handle(handle)?;
    let username = format!("{handle}@{domain}");
    if username.len() > MAX_USERNAME_BYTES {
        return Err(NameError::TooLong {
            len: username.len(),
        });
    }
    Ok(username)
}

/// The bare handle when `username` is V2 and its domain is one `accepted`; otherwise the whole
/// string (a legacy name, a foreign server's member, or something unparseable).
pub fn display_handle<'a>(username: &'a str, accepted: &[CanonicalDomain]) -> &'a str {
    match parse_username(username) {
        Ok(Username::V2 { handle, domain }) if accepted.iter().any(|d| d.as_str() == domain) => {
            handle
        }
        _ => username,
    }
}
