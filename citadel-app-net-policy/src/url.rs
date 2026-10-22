//! The one URL shape an app may ask for: `scheme://host[:port][/path][?query]`, strictly.
//!
//! Strict because every lenient corner of URL parsing is an SSRF bypass somewhere: userinfo
//! (`https://allowed@evil`), backslashes, fragments, whitespace, IPv4 in octal or hex
//! (`0x7f.1`), trailing dots. Anything this does not recognise is refused, not repaired.

use std::net::Ipv4Addr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    Https,
    Http,
}

impl Scheme {
    pub fn default_port(self) -> u16 {
        match self {
            Self::Https => 443,
            Self::Http => 80,
        }
    }
}

/// Why a URL or host is not one the policy reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlFault {
    TooLong,
    /// A byte outside printable ASCII, a space, or a backslash.
    Character,
    Scheme,
    /// `user:pass@` before the host.
    Credentials,
    Fragment,
    Host,
    Port,
}

/// A parsed target. `host` is lowercase; `port` is the explicit port or the scheme's default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetUrl {
    pub scheme: Scheme,
    pub host: String,
    pub port: u16,
    /// Starts with `/`; `/` when the URL had no path.
    pub path_and_query: String,
}

impl TargetUrl {
    pub fn parse(text: &str) -> Result<Self, UrlFault> {
        if text.len() > crate::MAX_URL_BYTES {
            return Err(UrlFault::TooLong);
        }
        if !text.bytes().all(|b| b.is_ascii_graphic() && b != b'\\') {
            return Err(UrlFault::Character);
        }
        if text.contains('#') {
            return Err(UrlFault::Fragment);
        }
        let (scheme, rest) = if let Some(rest) = text.strip_prefix("https://") {
            (Scheme::Https, rest)
        } else if let Some(rest) = text.strip_prefix("http://") {
            (Scheme::Http, rest)
        } else {
            return Err(UrlFault::Scheme);
        };
        let end = rest.find(['/', '?']).unwrap_or(rest.len());
        let (authority, tail) = rest.split_at(end);
        if authority.contains('@') {
            return Err(UrlFault::Credentials);
        }
        let (host, port) = split_host_port(authority)?;
        let path_and_query = match tail.strip_prefix('?') {
            Some(query) => format!("/?{query}"),
            None if tail.is_empty() => "/".to_string(),
            None => tail.to_string(),
        };
        Ok(Self {
            scheme,
            port: port.unwrap_or(scheme.default_port()),
            host,
            path_and_query,
        })
    }

    /// The literal IPv4 address the host is, if it is one.
    pub fn ip_literal(&self) -> Option<Ipv4Addr> {
        canonical_ipv4(&self.host)
    }
}

/// `host` or `host:port`, the host checked and lowercased.
pub(crate) fn split_host_port(authority: &str) -> Result<(String, Option<u16>), UrlFault> {
    if authority.starts_with('[') {
        return Err(UrlFault::Host);
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, Some(parse_port(port)?)),
        None => (authority, None),
    };
    Ok((parse_host(host)?, port))
}

fn parse_port(text: &str) -> Result<u16, UrlFault> {
    let digits = !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    if !digits || (text.len() > 1 && text.starts_with('0')) {
        return Err(UrlFault::Port);
    }
    match text.parse::<u16>() {
        Ok(0) | Err(_) => Err(UrlFault::Port),
        Ok(port) => Ok(port),
    }
}

/// A DNS name of letter-digit-hyphen labels, or a dotted-quad IPv4 address in canonical
/// decimal. IPv6 literals are not accepted. Lowercased.
fn parse_host(text: &str) -> Result<String, UrlFault> {
    let host = text.to_ascii_lowercase();
    if canonical_ipv4(&host).is_some() {
        return Ok(host);
    }
    if host.is_empty() || host.len() > 253 {
        return Err(UrlFault::Host);
    }
    let labels: Vec<&str> = host.split('.').collect();
    let label_ok = |label: &&str| {
        let bytes = label.as_bytes();
        !bytes.is_empty()
            && bytes.len() <= 63
            && bytes[0] != b'-'
            && bytes[bytes.len() - 1] != b'-'
            && bytes
                .iter()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
    };
    if !labels.iter().all(label_ok) {
        return Err(UrlFault::Host);
    }
    // No top-level name is numeric, and resolvers read `0x7f.1` or `2130706433` as addresses.
    let last = labels[labels.len() - 1];
    if !last.bytes().any(|b| b.is_ascii_lowercase()) || last.starts_with("0x") {
        return Err(UrlFault::Host);
    }
    Ok(host)
}

/// `a.b.c.d`, each part decimal 0..=255 without a leading zero.
fn canonical_ipv4(text: &str) -> Option<Ipv4Addr> {
    let parts: Vec<&str> = text.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut octets = [0u8; 4];
    for (octet, part) in octets.iter_mut().zip(&parts) {
        let decimal = !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
        if !decimal || (part.len() > 1 && part.starts_with('0')) {
            return None;
        }
        *octet = part.parse().ok()?;
    }
    Some(Ipv4Addr::from(octets))
}
