//! What a `Register` request's `server_addr` names.
//!
//! Three shapes arrive from the UI:
//!
//! - `ws://…` / `wss://…`: a server reached by WebSocket URL, dialled as given.
//! - `<org>.work.avarok.net` with no port: a hosted workspace, a Cloudflare Worker behind the
//!   edge on 443, which has no port of its own; dialled as `wss://<org>.work.avarok.net/`.
//! - anything else (`host:port`, `ip:port`): a server with a socket of its own, resolved and
//!   dialled over TCP exactly as before.

use citadel_sdk::prelude::WebSocketEndpoint;

/// Hosted workspaces live exactly one DNS label under this suffix.
const TENANT_HOST_SUFFIX: &str = ".work.avarok.net";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ServerAddress {
    WebSocket(WebSocketEndpoint),
    HostPort(String),
}

pub(crate) fn classify(input: &str) -> Result<ServerAddress, String> {
    let trimmed = input.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("ws://") || lower.starts_with("wss://") {
        return WebSocketEndpoint::parse(trimmed)
            .map(ServerAddress::WebSocket)
            .map_err(|err| err.into_string());
    }
    if is_tenant_host(&lower) {
        return WebSocketEndpoint::parse(&format!("wss://{lower}/"))
            .map(ServerAddress::WebSocket)
            .map_err(|err| err.into_string());
    }
    Ok(ServerAddress::HostPort(trimmed.to_string()))
}

/// One DNS label (letters, digits, inner hyphens) directly under the tenant suffix, no port.
fn is_tenant_host(lower: &str) -> bool {
    let Some(label) = lower.strip_suffix(TENANT_HOST_SUFFIX) else {
        return false;
    };
    !label.is_empty()
        && label.len() <= 63
        && label
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !label.starts_with('-')
        && !label.ends_with('-')
}

#[cfg(test)]
mod tests {
    use super::{classify, ServerAddress};

    fn websocket(input: &str) -> String {
        match classify(input).unwrap() {
            ServerAddress::WebSocket(endpoint) => endpoint.to_string(),
            other => panic!("{input:?} should be a WebSocket endpoint, was {other:?}"),
        }
    }

    #[test]
    fn a_bare_tenant_host_is_dialled_over_wss() {
        assert_eq!(
            websocket("acme.work.avarok.net"),
            "wss://acme.work.avarok.net/"
        );
        assert_eq!(
            websocket(" Acme-2.WORK.avarok.net "),
            "wss://acme-2.work.avarok.net/"
        );
    }

    #[test]
    fn websocket_urls_are_dialled_as_given() {
        assert_eq!(
            websocket("ws://localhost:8787/acme"),
            "ws://localhost:8787/acme"
        );
        assert_eq!(
            websocket("wss://acme.work.avarok.net/"),
            "wss://acme.work.avarok.net/"
        );
    }

    #[test]
    fn everything_else_is_host_port_as_before() {
        for input in [
            "127.0.0.1:12349",
            "citadel.avarok.net:12400",
            "acme.work.avarok.net:12400",
            "work.avarok.net",
            "a.b.work.avarok.net",
            "acme.work.avarok.net.evil.com",
            "-acme.work.avarok.net",
        ] {
            assert_eq!(
                classify(input).unwrap(),
                ServerAddress::HostPort(input.to_string()),
                "{input:?}"
            );
        }
    }

    #[test]
    fn a_malformed_websocket_url_is_refused_not_dialled_over_tcp() {
        assert!(classify("wss://").is_err());
        assert!(classify("wss://user:pw@acme.work.avarok.net/").is_err());
    }
}
