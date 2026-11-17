//! The browser `Origin` each control connection's handshake carried
//! (`sign-in-with-citadel.md` §8). The WebSocket interface records it while `OriginPolicy`
//! admits the handshake; the kernel files it here under the connection's id for the life of
//! the connection, and `IdentitySignChallenge` signs exactly this value. No request can set
//! or change it: a page names its issuer and audience, never its origin.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Default)]
pub(crate) struct ControlOrigins(RwLock<HashMap<Uuid, String>>);

impl ControlOrigins {
    /// Records `origin` for `conn` until the returned entry is dropped (the connection's
    /// handler task holds it, so the record cannot outlive the connection). A connection
    /// without an `Origin` (a native client) records nothing.
    pub(crate) fn admit(self: &Arc<Self>, conn: Uuid, origin: Option<&str>) -> OriginEntry {
        if let Some(origin) = origin {
            self.0.write().insert(conn, origin.to_string());
        }
        OriginEntry {
            origins: self.clone(),
            conn,
        }
    }

    /// The `Origin` `conn`'s handshake carried; `None` for a native client or a closed one.
    pub(crate) fn origin(&self, conn: &Uuid) -> Option<String> {
        self.0.read().get(conn).cloned()
    }
}

/// A connection's record in `ControlOrigins`, removed on drop.
pub(crate) struct OriginEntry {
    origins: Arc<ControlOrigins>,
    conn: Uuid,
}

impl Drop for OriginEntry {
    fn drop(&mut self) {
        self.origins.0.write().remove(&self.conn);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_origin_lives_exactly_as_long_as_its_connection() {
        let origins = Arc::new(ControlOrigins::default());
        let (browser, native) = (Uuid::new_v4(), Uuid::new_v4());
        let entry = origins.admit(browser, Some("https://work.avarok.net"));
        let _native = origins.admit(native, None);
        assert_eq!(
            origins.origin(&browser).as_deref(),
            Some("https://work.avarok.net")
        );
        assert_eq!(origins.origin(&native), None);
        drop(entry);
        assert_eq!(
            origins.origin(&browser),
            None,
            "a closed connection keeps nothing"
        );
    }
}
