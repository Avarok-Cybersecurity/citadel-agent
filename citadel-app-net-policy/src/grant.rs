//! What an install was granted, and the deployment's own addresses.

use crate::url::{split_host_port, UrlFault};
use crate::{DEFAULT_PORTS, MAX_HOSTS};

/// Where the request leaves from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Egress {
    /// The member's own machine, through their local agent (`net.host`). `private` is
    /// `net.host.private`: http, and private, loopback and link-local addresses.
    Host { private: bool },
    /// The workspace server (`net.server`). Never private addresses, never http.
    Server,
}

/// One allow-listed host: an exact name (no wildcards), and the one port it may be reached on,
/// or `DEFAULT_PORTS` when the entry names none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowedHost {
    pub host: String,
    pub port: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantFault {
    NoHosts,
    TooManyHosts,
    /// A `*` anywhere in an entry.
    Wildcard(String),
    Malformed(String),
    Duplicate(String),
}

impl AllowedHost {
    /// `api.acme.com` or `api.acme.com:8080`. Nothing else: no scheme, path or wildcard.
    pub fn parse(entry: &str) -> Result<Self, GrantFault> {
        if entry.contains('*') {
            return Err(GrantFault::Wildcard(entry.to_string()));
        }
        let malformed = |_: UrlFault| GrantFault::Malformed(entry.to_string());
        if entry.contains(['/', '@', '?', '#']) || entry != entry.to_ascii_lowercase() {
            return Err(GrantFault::Malformed(entry.to_string()));
        }
        let (host, port) = split_host_port(entry).map_err(malformed)?;
        Ok(Self { host, port })
    }

    pub fn admits(&self, host: &str, port: u16) -> bool {
        self.host == host
            && match self.port {
                Some(listed) => listed == port,
                None => DEFAULT_PORTS.contains(&port),
            }
    }
}

/// An install's network grant: where from, and to which hosts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetGrant {
    pub egress: Egress,
    hosts: Vec<AllowedHost>,
}

impl NetGrant {
    /// One to `MAX_HOSTS` distinct, explicit entries.
    pub fn new(egress: Egress, entries: &[&str]) -> Result<Self, GrantFault> {
        if entries.is_empty() {
            return Err(GrantFault::NoHosts);
        }
        if entries.len() > MAX_HOSTS {
            return Err(GrantFault::TooManyHosts);
        }
        let mut hosts: Vec<AllowedHost> = Vec::with_capacity(entries.len());
        for entry in entries {
            let host = AllowedHost::parse(entry)?;
            if hosts.contains(&host) {
                return Err(GrantFault::Duplicate(entry.to_string()));
            }
            hosts.push(host);
        }
        Ok(Self { egress, hosts })
    }

    pub fn hosts(&self) -> &[AllowedHost] {
        &self.hosts
    }

    pub fn private(&self) -> bool {
        matches!(self.egress, Egress::Host { private: true })
    }

    pub fn admits(&self, host: &str, port: u16) -> bool {
        self.hosts.iter().any(|entry| entry.admits(host, port))
    }
}

/// The deployment's own addresses, which no grant reaches. Required, not defaulted: a host that
/// does not know its page origin has not been configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deployment {
    /// The host the page is served from (`acme.work.avarok.net`, or `localhost` in a dev
    /// stack), lowercase, no port.
    pub page_origin_host: String,
}
