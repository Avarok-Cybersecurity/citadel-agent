//! Whether a login may displace a session the server still holds for the account.
//!
//! Since Citadel-Protocol #319 the server replaces a session it holds for the account when a new
//! login with `force_login` fully authenticates, instead of refusing it ("Session Already
//! Connected") until its keep-alive expires -- up to an hour after a one-sided drop.
//!
//! A user signing in forces: this agent reaches the server only when it holds no live session for
//! the account (a live one answers `SessionAlreadyActive` first), so whatever the server holds is
//! stale or on another device, and the latest deliberate sign-in wins.
//!
//! The agent's own reconnect does NOT force. The server gives a displaced client no reason, so a
//! reconnect that forced could not tell "my session went half-open" from "I was replaced by a
//! sign-in on another device" -- and two agents on one account would displace each other forever.
//! It waits out the server's stale-session window instead (`kernel::reconnect`), and the user can
//! end the wait by signing in.
use citadel_sdk::prelude::ConnectMode;

/// Who is asking the server for the session.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum LoginOrigin {
    /// The user signed in (a Connect request).
    UserSignIn,
    /// The agent is restoring a session its server connection lost.
    AutomaticReconnect,
}

/// The mode to send: the caller's variant (Standard or Fetch), with `force_login` set by origin.
pub(crate) fn server_connect_mode(requested: ConnectMode, origin: LoginOrigin) -> ConnectMode {
    let force_login: bool = origin == LoginOrigin::UserSignIn;
    match requested {
        ConnectMode::Standard { .. } => ConnectMode::Standard { force_login },
        ConnectMode::Fetch { .. } => ConnectMode::Fetch { force_login },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_fetch(mode: &ConnectMode) -> bool {
        matches!(mode, ConnectMode::Fetch { .. })
    }

    #[test]
    fn a_user_sign_in_forces_whatever_the_ui_sent() {
        for requested in [
            ConnectMode::Standard { force_login: false },
            ConnectMode::Standard { force_login: true },
        ] {
            let sent: ConnectMode = server_connect_mode(requested, LoginOrigin::UserSignIn);
            assert!(sent.force_login() && !is_fetch(&sent));
        }
        let sent: ConnectMode = server_connect_mode(
            ConnectMode::Fetch { force_login: false },
            LoginOrigin::UserSignIn,
        );
        assert!(sent.force_login() && is_fetch(&sent), "the variant is kept");
    }

    #[test]
    fn an_automatic_reconnect_never_forces() {
        for requested in [
            ConnectMode::Standard { force_login: true },
            ConnectMode::Fetch { force_login: true },
        ] {
            let was_fetch: bool = is_fetch(&requested);
            let sent: ConnectMode = server_connect_mode(requested, LoginOrigin::AutomaticReconnect);
            assert!(!sent.force_login());
            assert_eq!(is_fetch(&sent), was_fetch, "the variant is kept");
        }
    }
}
