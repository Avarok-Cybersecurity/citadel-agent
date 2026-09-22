//! The server host (`host[:port]`) an account registered to, as the user typed it.
//!
//! The CNAC records a server as a resolved `SocketAddr`. For a hosted tenant
//! (`acme.work.avarok.net`) that is one of the edge's anycast addresses, shared by
//! every workspace behind it, so it cannot tell a user which workspace an account
//! belongs to. The typed host can, so the agent keeps it beside the account.
//!
//! Stored in the account's own byte map in the SDK persistence backend, under a key
//! of the agent's own. The byte map lives in the CNAC, so it is written to disk with
//! the filesystem backend, survives an agent restart, and is deleted with the
//! account on deregister. The key is distinct from the SDK key-value store's
//! (`_INTERNAL_DATA_MAP`, which is what `LocalDB*` requests read and write), so no
//! client can overwrite it or wipe it with `LocalDBClearAllKV`.

use crate::kernel::server_address::ServerAddress;
use citadel_sdk::prelude::{NodeRemote, Ratchet};
use std::net::{Ipv4Addr, Ipv6Addr};

const KEY: &str = "_AGENT_SERVER_HOST";
const SUB_KEY: &str = "host";
/// The account's own entries, as opposed to any peer's.
const SELF_PEER_CID: u64 = 0;
/// A 253-character DNS name plus `:65535`.
pub(crate) const MAX_LEN: usize = 259;
const MAX_DNS_NAME_LEN: usize = 253;
const MAX_LABEL_LEN: usize = 63;

/// The `host[:port]` to record for a registration to `address`, validated.
///
/// A WebSocket endpoint is recorded as its host, with the port only when it is not
/// the scheme's default, so `acme.work.avarok.net` is recorded as typed.
pub(crate) fn from_typed(address: &ServerAddress) -> Result<String, String> {
    let host = match address {
        ServerAddress::HostPort(host_port) => host_port.trim().to_ascii_lowercase(),
        ServerAddress::WebSocket(endpoint) => {
            let host = endpoint.host().to_ascii_lowercase();
            let host = if host.parse::<Ipv6Addr>().is_ok() {
                format!("[{host}]")
            } else {
                host
            };
            let default_port = if endpoint.is_secure() { 443 } else { 80 };
            if endpoint.port() == default_port {
                host
            } else {
                format!("{host}:{}", endpoint.port())
            }
        }
    };
    validate(&host)?;
    Ok(host)
}

/// Accepts `host`, `host:port`, `ip:port` and `[ipv6]:port`; nothing with a scheme,
/// path, userinfo, whitespace or an unbracketed IPv6 address.
pub(crate) fn validate(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("server host is empty".to_string());
    }
    if value.len() > MAX_LEN {
        return Err(format!(
            "server host is {} bytes; at most {MAX_LEN} are allowed",
            value.len()
        ));
    }
    let (host, port) = if let Some(rest) = value.strip_prefix('[') {
        let (inside, after) = rest
            .split_once(']')
            .ok_or_else(|| format!("{value:?} opens an IPv6 bracket it never closes"))?;
        inside
            .parse::<Ipv6Addr>()
            .map_err(|_| format!("{inside:?} is not an IPv6 address"))?;
        let port = match after {
            "" => None,
            _ => Some(after.strip_prefix(':').ok_or_else(|| {
                format!("{value:?} has {after:?} after its IPv6 address, not a port")
            })?),
        };
        (None, port)
    } else {
        match value.split_once(':') {
            Some((host, port)) => (Some(host), Some(port)),
            None => (Some(value), None),
        }
    };
    if let Some(port) = port {
        let valid = !port.is_empty()
            && port.bytes().all(|b| b.is_ascii_digit())
            && port.parse::<u16>().is_ok_and(|port| port != 0);
        if !valid {
            return Err(format!("{port:?} in {value:?} is not a port (1-65535)"));
        }
    }
    match host {
        None => Ok(()),
        Some(host) if host.parse::<Ipv4Addr>().is_ok() || is_dns_name(host) => Ok(()),
        Some(host) => Err(format!("{host:?} is not a host name or IP address")),
    }
}

fn is_dns_name(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= MAX_DNS_NAME_LEN
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= MAX_LABEL_LEN
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
}

pub(crate) async fn store<R: Ratchet>(
    remote: &NodeRemote<R>,
    cid: u64,
    host: &str,
) -> Result<(), String> {
    validate(host)?;
    remote
        .account_manager()
        .get_persistence_handler()
        .store_byte_map_value(cid, SELF_PEER_CID, KEY, SUB_KEY, host.as_bytes().to_vec())
        .await
        .map(|_previous| ())
        .map_err(|err| err.into_string())
}

/// The recorded host of account `cid`, or `None` for an account registered before
/// the agent recorded one.
pub(crate) async fn load<R: Ratchet>(
    remote: &NodeRemote<R>,
    cid: u64,
) -> Result<Option<String>, String> {
    let stored = remote
        .account_manager()
        .get_persistence_handler()
        .get_byte_map_value(cid, SELF_PEER_CID, KEY, SUB_KEY)
        .await
        .map_err(|err| err.into_string())?;
    let Some(bytes) = stored else {
        return Ok(None);
    };
    let host = String::from_utf8(bytes)
        .map_err(|err| format!("stored server host for {cid} is not UTF-8: {err}"))?;
    validate(&host).map_err(|err| format!("stored server host for {cid} is invalid: {err}"))?;
    Ok(Some(host))
}

#[cfg(test)]
#[path = "server_host_tests.rs"]
mod tests;
