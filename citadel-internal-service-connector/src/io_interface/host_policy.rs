//! Which `Host` header a handshake may carry.
//!
//! The loopback certificate makes the agent reachable as a NAME -- `wss://local.example.com:12345`,
//! a public name the operator points at 127.0.0.1 -- so that a page served over HTTPS may open
//! the socket at all. A name changes what "this socket" means: DNS rebinding resolves an
//! attacker's `evil.example` to 127.0.0.1 for one request, and the browser then sends
//! `Host: evil.example:12345` to this very listener. The Origin allowlist refuses that page's
//! Origin already; this refuses the request on its Host as well, so a socket reachable by a name
//! is reachable by exactly the names the operator meant, and nothing a resolver can invent.
//!
//! [`HostPolicy::Any`] is the plain listener's behaviour, unchanged: behind the UI's nginx proxy
//! the Host header is the UI's own, and the proxy validates it before forwarding.

/// The Hosts a handshake may name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostPolicy {
    /// Every Host. The plain listener behind a proxy that validates Host itself.
    Any,
    /// The agent's own socket: `127.0.0.1`, `localhost` and `[::1]` on exactly this port, plus
    /// the one published name, if any. The port is REQUIRED in the header -- a browser always
    /// sends it for a non-default port, and a Host without one is not this socket.
    Loopback { port: u16, name: Option<String> },
}

impl HostPolicy {
    /// The loopback policy for a listener on `port`, optionally reachable as `name`.
    pub fn loopback(port: u16, name: Option<&str>) -> Self {
        let name = name
            .map(|n| n.trim().to_ascii_lowercase())
            .filter(|n| !n.is_empty());
        Self::Loopback { port, name }
    }

    /// Whether a handshake carrying this `Host` header may proceed. Compared case-insensitively
    /// on the name, exactly on the port.
    pub fn permits(&self, host: Option<&str>) -> bool {
        match self {
            Self::Any => true,
            Self::Loopback { port, name } => {
                let Some(host) = host else { return false };
                let host = host.trim().to_ascii_lowercase();
                let Some((host_name, Some(host_port))) = split_host_port(&host) else {
                    return false;
                };
                if host_port != *port {
                    return false;
                }
                matches!(host_name, "127.0.0.1" | "localhost" | "[::1]")
                    || name.as_deref() == Some(host_name)
            }
        }
    }
}

/// `host[:port]`, with bracketed IPv6 literals kept whole. `None` when the port is not a number.
fn split_host_port(host: &str) -> Option<(&str, Option<u16>)> {
    if host.starts_with('[') {
        let end = host.find(']')?;
        let name = &host[..=end];
        return match host[end + 1..].strip_prefix(':') {
            Some(port) => Some((name, Some(port.parse().ok()?))),
            None => Some((name, None)),
        };
    }
    match host.rsplit_once(':') {
        Some((name, port)) => Some((name, Some(port.parse().ok()?))),
        None => Some((host, None)),
    }
}

#[cfg(test)]
mod tests {
    use super::HostPolicy;

    #[test]
    fn any_permits_everything_including_no_header() {
        assert!(HostPolicy::Any.permits(Some("evil.example:1")));
        assert!(HostPolicy::Any.permits(None));
    }

    #[test]
    fn loopback_permits_the_three_loopback_names_on_the_exact_port() {
        let p = HostPolicy::loopback(12345, None);
        for h in [
            "127.0.0.1:12345",
            "localhost:12345",
            "[::1]:12345",
            "LOCALHOST:12345",
        ] {
            assert!(p.permits(Some(h)), "{h}");
        }
    }

    #[test]
    fn loopback_refuses_other_ports_missing_ports_other_names_and_no_header() {
        let p = HostPolicy::loopback(12345, None);
        for h in [
            "127.0.0.1:12346",
            "127.0.0.1",
            "localhost",
            "evil.example:12345",
            "[::1]",
            "127.0.0.1:x",
        ] {
            assert!(!p.permits(Some(h)), "{h}");
        }
        assert!(!p.permits(None));
    }

    #[test]
    fn the_published_name_is_permitted_and_only_that_name() {
        let p = HostPolicy::loopback(12345, Some("Local.Example.com"));
        assert!(p.permits(Some("local.example.com:12345")));
        assert!(p.permits(Some("LOCAL.example.COM:12345")));
        assert!(!p.permits(Some("local.example.com:12346")));
        assert!(!p.permits(Some("local.example.com")));
        assert!(!p.permits(Some("evil.example.com:12345")));
        assert!(!p.permits(Some("xlocal.example.com:12345")));
    }

    #[test]
    fn an_empty_published_name_is_no_name() {
        assert_eq!(
            HostPolicy::loopback(1, Some("  ")),
            HostPolicy::loopback(1, None)
        );
    }
}
