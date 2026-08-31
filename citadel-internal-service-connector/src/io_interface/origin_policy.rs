//! Which browser origins may open a control connection to this agent.
//!
//! The agent listens on a WebSocket and, until this existed, accepted every
//! handshake that arrived. A WebSocket is exempt from the same-origin policy
//! and from CORS preflight, so ANY page a user happened to visit could open
//! `ws://localhost:12345`, enumerate every account with `GetSessions`, and then
//! act as them. That is the enabler behind the whole ownership-gate class: a
//! per-command check cannot help if the caller should never have been on the
//! socket.
//!
//! # What this controls, and what it does not
//!
//! A browser sets `Origin` on the WebSocket handshake itself and page script
//! cannot change or suppress it. So for the browser threat — a hostile page —
//! this is a real boundary.
//!
//! It is **not** a boundary against a native process on the same machine. Such
//! a process writes whatever `Origin` it likes, or none. `permits(None)` is
//! therefore `true` under `Allow`: refusing header-less handshakes would lock
//! out legitimate native clients while stopping no attacker who can already run
//! code locally. Keeping a hostile local *process* off this socket is an
//! OS-level problem (loopback binding, socket permissions, a bearer token) and
//! is deliberately out of scope here. Do not read a passing test in this module
//! as evidence of that stronger property.

/// The configured decision. There is no `Default`: an operator must say which
/// origins are theirs, because the safe-looking guess ("localhost") is wrong
/// for every real deployment and the convenient guess ("any") is the hole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OriginPolicy {
    /// Only these exact origins, compared byte-for-byte.
    Allow(Vec<String>),
    /// Every origin. Explicit opt-in only — `*` in the configuration.
    Any,
}

impl OriginPolicy {
    /// Parse a comma-separated specification.
    ///
    /// `*` alone means [`OriginPolicy::Any`]. Anything else is a list of exact
    /// origins, e.g. `http://localhost:5291,https://app.example.com`.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let entries: Vec<String> = spec
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(str::to_string)
            .collect();

        if entries.is_empty() {
            return Err(
                "no origins given: pass an explicit list, or `*` to accept any origin".to_string(),
            );
        }

        if entries.iter().any(|entry| entry == "*") {
            if entries.len() > 1 {
                // `*,http://localhost:5291` reads as "these, plus a wildcard",
                // which is the same as the wildcard. Refusing it stops an
                // operator believing a list is in force when it is not.
                return Err(
                    "`*` cannot be combined with specific origins: it already allows them"
                        .to_string(),
                );
            }
            return Ok(OriginPolicy::Any);
        }

        // A trailing slash is the classic mismatch: browsers send
        // `http://host:port` with no path, so `http://host:port/` never
        // matches and the failure looks like the agent being down.
        if let Some(bad) = entries.iter().find(|entry| entry.ends_with('/')) {
            return Err(format!(
                "origin `{bad}` has a trailing slash; browsers send no path component"
            ));
        }

        if let Some(bad) = entries
            .iter()
            .find(|entry| !entry.starts_with("http://") && !entry.starts_with("https://"))
        {
            return Err(format!(
                "origin `{bad}` has no scheme; an Origin header is always scheme://host[:port]"
            ));
        }

        Ok(OriginPolicy::Allow(entries))
    }

    /// May a handshake carrying this `Origin` proceed?
    pub fn permits(&self, origin: Option<&str>) -> bool {
        match self {
            OriginPolicy::Any => true,
            // See the module docs: absent means "not a browser", which this
            // control does not claim to police.
            OriginPolicy::Allow(_) if origin.is_none() => true,
            OriginPolicy::Allow(allowed) => {
                let origin = origin.expect("checked immediately above");
                allowed.iter().any(|entry| entry == origin)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow(spec: &str) -> OriginPolicy {
        OriginPolicy::parse(spec).expect("valid spec")
    }

    #[test]
    fn a_listed_origin_is_admitted() {
        assert!(allow("http://localhost:5291").permits(Some("http://localhost:5291")));
    }

    /// The whole point: a page the user merely visited.
    #[test]
    fn an_unlisted_origin_is_refused() {
        assert!(!allow("http://localhost:5291").permits(Some("https://evil.example")));
    }

    #[test]
    fn the_comparison_is_exact() {
        let policy = allow("http://localhost:5291");
        // A subdomain-ish prefix, a port change, and a scheme change are three
        // different origins. Any substring or prefix match here would admit all
        // three.
        assert!(!policy.permits(Some("http://localhost:5291.evil.example")));
        assert!(!policy.permits(Some("http://localhost:5292")));
        assert!(!policy.permits(Some("https://localhost:5291")));
    }

    #[test]
    fn several_origins_may_be_listed() {
        let policy = allow("http://localhost:5291, http://127.0.0.1:5291");
        assert!(policy.permits(Some("http://localhost:5291")));
        assert!(policy.permits(Some("http://127.0.0.1:5291")));
        assert!(!policy.permits(Some("http://localhost:4173")));
    }

    /// Documented limit, pinned so nobody later reads this control as stronger
    /// than it is. See the module docs.
    #[test]
    fn a_handshake_with_no_origin_is_not_policed() {
        assert!(allow("http://localhost:5291").permits(None));
    }

    #[test]
    fn the_wildcard_admits_everything_including_no_origin() {
        assert_eq!(OriginPolicy::parse("*"), Ok(OriginPolicy::Any));
        assert!(OriginPolicy::Any.permits(Some("https://evil.example")));
        assert!(OriginPolicy::Any.permits(None));
    }

    #[test]
    fn an_empty_specification_is_an_error_not_an_empty_allowlist() {
        // An empty allowlist would refuse the UI while looking configured.
        assert!(OriginPolicy::parse("").is_err());
        assert!(OriginPolicy::parse("  , ,").is_err());
    }

    #[test]
    fn the_wildcard_cannot_hide_inside_a_list() {
        let error = OriginPolicy::parse("http://localhost:5291,*").unwrap_err();
        assert!(
            error.contains('*'),
            "the message should name the problem: {error}"
        );
    }

    #[test]
    fn a_trailing_slash_is_rejected_at_parse_time() {
        // Rather than at 3am, as "the agent is down".
        let error = OriginPolicy::parse("http://localhost:5291/").unwrap_err();
        assert!(error.contains("trailing slash"), "{error}");
    }

    #[test]
    fn a_bare_host_is_rejected_at_parse_time() {
        let error = OriginPolicy::parse("localhost:5291").unwrap_err();
        assert!(error.contains("scheme"), "{error}");
    }
}
