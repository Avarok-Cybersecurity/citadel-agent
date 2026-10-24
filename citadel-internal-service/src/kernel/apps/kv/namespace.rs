//! Where one install's `local` entries live in the account's LocalDB, built by the agent.
//!
//! `app:<app_id>:<hex scope_key>:<hex username>:` then `i` for the index, or `k:<hex key>` for
//! an entry. The app id's grammar has no `:`; the scope key (`node:<id>`, `self:<name>`, ...)
//! and the username may, so both are hex-encoded, as is every key: a component can never
//! end early or carry a separator, `..` means nothing to a flat store, and no key spelling
//! reaches another namespace or a store the agent keeps for anything else.

use citadel_ihttp_types::MAX_SCOPE_KEY_BYTES;
use citadel_internal_service_types::{AppKvRefusal, APP_KV_MAX_KEY_BYTES};

/// Every app record's key starts with this; nothing else the agent stores does, and no
/// window's LocalDB request may name one (requests/local_db).
pub(crate) const APP_STORAGE_PREFIX: &str = "app:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Namespace {
    prefix: String,
}

impl Namespace {
    pub(crate) fn new(app_id: &str, scope_key: &str, username: &str) -> Result<Self, AppKvRefusal> {
        citadel_app_id::validate_app_id(app_id).map_err(|_| AppKvRefusal::Namespace)?;
        let scope_ok = !scope_key.is_empty()
            && scope_key.len() <= MAX_SCOPE_KEY_BYTES
            && scope_key.bytes().all(|b| b.is_ascii_graphic());
        if !scope_ok || username.is_empty() {
            return Err(AppKvRefusal::Namespace);
        }
        Ok(Self {
            prefix: format!(
                "{APP_STORAGE_PREFIX}{app_id}:{}:{}:",
                hex::encode(scope_key),
                hex::encode(username)
            ),
        })
    }

    /// The index record: every key in the namespace and the size of its value.
    pub(crate) fn index(&self) -> String {
        format!("{}i", self.prefix)
    }

    /// Where `key`'s value is stored. Any non-empty key of at most `APP_KV_MAX_KEY_BYTES`.
    pub(crate) fn entry(&self, key: &str) -> Result<String, AppKvRefusal> {
        if key.is_empty() || key.len() > APP_KV_MAX_KEY_BYTES {
            return Err(AppKvRefusal::Key);
        }
        Ok(format!("{}k:{}", self.prefix, hex::encode(key)))
    }

    /// Whether `stored` is a record of this namespace.
    #[cfg(test)]
    pub(crate) fn owns(&self, stored: &str) -> bool {
        stored.starts_with(&self.prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ns(app: &str, scope: &str, user: &str) -> Namespace {
        Namespace::new(app, scope, user).expect("valid")
    }

    #[test]
    fn a_crafted_key_never_leaves_its_namespace() {
        let mine = ns("com.acme.kanban", "workspace", "alice");
        let others = [
            ns("com.acme.kanban", "workspace", "bob"),
            ns("com.acme.kanban", "node:n1", "alice"),
            ns("com.acme.other", "workspace", "alice"),
        ];
        for crafted in [
            "i",
            "../../i",
            "..",
            ":i",
            "x:k:62",
            "/etc/passwd",
            "k:",
            "\u{0}",
            "a\n:6",
            "app:com.acme.kanban:776f726b7370616365:626f62:k:6b",
        ] {
            let stored = mine.entry(crafted).expect(crafted);
            assert!(mine.owns(&stored), "{crafted}");
            assert_ne!(stored, mine.index(), "{crafted} named the index");
            for other in &others {
                assert!(!other.owns(&stored), "{crafted} reached {other:?}");
            }
        }
    }

    #[test]
    fn components_that_hold_the_separator_cannot_collide() {
        // Unencoded, both would be `app:com.acme.kanban:node:a:b:...`.
        let left = ns("com.acme.kanban", "node:a", "b:c");
        let right = ns("com.acme.kanban", "node:a:b", "c");
        assert_ne!(left.index(), right.index());
        assert!(!left.owns(&right.entry("k").unwrap()));
        assert!(!right.owns(&left.entry("k").unwrap()));
    }

    #[test]
    fn a_namespace_needs_a_real_app_id_scope_key_and_account() {
        for (app, scope, user) in [
            ("com:acme", "workspace", "alice"),
            ("nodots", "workspace", "alice"),
            ("com.acme.kanban", "", "alice"),
            ("com.acme.kanban", "has space", "alice"),
            (
                "com.acme.kanban",
                &"s".repeat(MAX_SCOPE_KEY_BYTES + 1),
                "alice",
            ),
            ("com.acme.kanban", "workspace", ""),
        ] {
            assert_eq!(
                Namespace::new(app, scope, user),
                Err(AppKvRefusal::Namespace),
                "{app} {scope} {user}"
            );
        }
        let ok = ns("com.acme.kanban", "workspace", "alice");
        assert_eq!(ok.entry(""), Err(AppKvRefusal::Key));
        assert_eq!(
            ok.entry(&"k".repeat(APP_KV_MAX_KEY_BYTES + 1)),
            Err(AppKvRefusal::Key)
        );
        assert!(ok.entry(&"k".repeat(APP_KV_MAX_KEY_BYTES)).is_ok());
    }
}
