//! The server's canonical domain: its identity (identity doc §2, §3.1 rev b).

use crate::MAX_DOMAIN_BYTES;
use std::fmt;
use std::net::Ipv6Addr;

/// A server's identity: lowercase host (an IPv6 literal bracketed and compressed), no scheme, no
/// `:443`, no trailing `/`, path kept.
/// Only `canonical_domain` makes one, so holding one means the string is already canonical.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CanonicalDomain(String);

impl CanonicalDomain {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CanonicalDomain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainError {
    Empty,
    TooLong {
        len: usize,
    },
    /// Whitespace, `@`, `?`, `#`, `%`, `\` or a non-ASCII byte: never part of a server name.
    ForbiddenChar(char),
    BadScheme,
    BadHost(String),
    BadPort(String),
    BadPath(String),
}

impl fmt::Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "the server name is empty"),
            Self::TooLong { len } => write!(
                f,
                "the server name is {len} bytes, over the {MAX_DOMAIN_BYTES} allowed"
            ),
            Self::ForbiddenChar(c) => write!(f, "{c:?} cannot be part of a server name"),
            Self::BadScheme => write!(f, "the scheme before \"://\" is malformed"),
            Self::BadHost(why) => write!(f, "the host is malformed: {why}"),
            Self::BadPort(port) => write!(f, "{port:?} is not a port"),
            Self::BadPath(why) => write!(f, "the path is malformed: {why}"),
        }
    }
}

impl std::error::Error for DomainError {}

/// The canonical form of what was dialled: lowercased, scheme, trailing slashes and a `:443`
/// removed, a path (two tenants on one host) kept. Idempotent.
pub fn canonical_domain(dialled: &str) -> Result<CanonicalDomain, DomainError> {
    if let Some(c) = dialled
        .chars()
        .find(|c| !c.is_ascii() || c.is_ascii_whitespace() || "@?#%\\".contains(*c))
    {
        return Err(DomainError::ForbiddenChar(c));
    }
    let lower = dialled.to_ascii_lowercase();
    let rest = match lower.split_once("://") {
        Some((scheme, rest)) => {
            let mut chars = scheme.chars();
            let first_ok = chars.next().is_some_and(|c| c.is_ascii_lowercase());
            if !first_ok || !chars.all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)) {
                return Err(DomainError::BadScheme);
            }
            rest
        }
        None => lower.as_str(),
    };
    let rest = rest.trim_end_matches('/');
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, Some(path)),
        None => (rest, None),
    };
    let (mut out, port) = split_authority(authority)?;
    if let Some(port) = port {
        let valid = !port.starts_with('0') && port.parse::<u16>().is_ok_and(|p| p != 0);
        if !valid || !port.bytes().all(|b| b.is_ascii_digit()) {
            return Err(DomainError::BadPort(port.to_string()));
        }
        if port != "443" {
            out.push(':');
            out.push_str(port);
        }
    }
    if let Some(path) = path {
        for segment in path.split('/') {
            check_path_segment(segment)?;
        }
        out.push('/');
        out.push_str(path);
    }
    if out.len() > MAX_DOMAIN_BYTES {
        return Err(DomainError::TooLong { len: out.len() });
    }
    Ok(CanonicalDomain(out))
}

/// The canonical host and the port text of `authority`.
///
/// A bracketed IPv6 literal, `[addr]` or `[addr]:port`, is parsed whole, so its colons are never
/// read as the port, and written as `Ipv6Addr`'s `Display` prints it (lowercase, compressed), so
/// every spelling of one address names one server and derives one key. An unbracketed IPv6
/// address is ambiguous with the port and stays refused. Names and IPv4 take the path they
/// always took: their output (and with it every existing account's keys) does not change.
fn split_authority(authority: &str) -> Result<(String, Option<&str>), DomainError> {
    let Some(bracketed) = authority.strip_prefix('[') else {
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        };
        check_host(host)?;
        return Ok((host.to_string(), port));
    };
    let (literal, after) = bracketed
        .split_once(']')
        .ok_or_else(|| DomainError::BadHost("'[' is never closed".to_string()))?;
    let address: Ipv6Addr = literal
        .parse()
        .map_err(|_| DomainError::BadHost(format!("{literal:?} is not an IPv6 address")))?;
    let port =
        match after {
            "" => None,
            _ => Some(after.strip_prefix(':').ok_or_else(|| {
                DomainError::BadHost(format!("{after:?} follows the IPv6 address"))
            })?),
        };
    Ok((format!("[{address}]"), port))
}

fn check_host(host: &str) -> Result<(), DomainError> {
    if host.is_empty() {
        return Err(DomainError::Empty);
    }
    for label in host.split('.') {
        let charset = label
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if label.is_empty() || label.len() > 63 || !charset {
            return Err(DomainError::BadHost(format!("label {label:?}")));
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(DomainError::BadHost(format!(
                "label {label:?} edges on '-'"
            )));
        }
    }
    Ok(())
}

fn check_path_segment(segment: &str) -> Result<(), DomainError> {
    let charset = segment
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._~-".contains(&b));
    if segment.is_empty() || segment == "." || segment == ".." || !charset {
        return Err(DomainError::BadPath(format!("segment {segment:?}")));
    }
    Ok(())
}
