//! What a window offers, as the SDK's `SignInFactors`, and the relay that serves the key
//! challenges they raise for as long as the request runs.

use super::key_relay::{Asker, KeyChallenges, KeyRelay};
use crate::kernel::reconnect::Reauth;
use crate::kernel::session_route::Clients;
use citadel_sdk::prelude::{
    RecoveryPhrase, SecBuffer, SecurityKeyPrf, SessionScope, SignInFactors,
};
use std::sync::Arc;

/// A sign-in or step-up under way: what its session's scope will be, and the relay serving
/// its key challenges. Dropping it closes them.
pub(crate) struct Underway {
    pub scope: SessionScope,
    relay: Option<KeyRelay>,
}

/// What a window's request offers: a `Connect`'s factors, or a step-up's.
pub(crate) struct Offer {
    pub password: Option<SecBuffer>,
    /// Whether the window can answer a key challenge.
    pub security_key: bool,
    /// The recovery phrase, read and bound to the account's server (`phrase::read`).
    pub recovery_phrase: Option<RecoveryPhrase>,
    /// A fresh sign-in's admission (Turnstile) token. Never logged.
    pub admission: Option<String>,
}

/// The factors for the SDK, offering the key channel only when the window can answer it.
pub(crate) fn begin(
    challenges: &Arc<KeyChallenges>,
    clients: &Clients,
    asker: Asker,
    offer: Offer,
) -> (SignInFactors, Underway) {
    let Offer {
        password,
        security_key,
        recovery_phrase,
        admission,
    } = offer;
    let (security_key, relay) = match security_key {
        true => {
            let (key, relay) = KeyRelay::start(challenges, clients, asker);
            (Some(key), Some(relay))
        }
        false => (None, None),
    };
    let offered = Offered {
        password,
        security_key,
        recovery_phrase,
    };
    let scope = offered.scope();
    let mut factors = offered.into_factors();
    factors.admission = admission;
    (factors, Underway { scope, relay })
}

impl Underway {
    /// The sign-in succeeded: how the session can be opened again (kernel/sign_in/mod.rs).
    pub(crate) fn finish(self, password: Option<SecBuffer>) -> Reauth {
        let key_asked = self.relay.as_ref().is_some_and(KeyRelay::asked);
        super::reauth(self.scope, key_asked, password)
    }
}

/// The factors a `Connect` or a step-up offers, before the SDK sees them.
struct Offered {
    pub password: Option<SecBuffer>,
    /// The relay's channel, when the window can answer a key challenge.
    pub security_key: Option<SecurityKeyPrf>,
    pub recovery_phrase: Option<RecoveryPhrase>,
}

impl Offered {
    /// The scope the session will have. A sign-in with the phrase is a recovery sign-in,
    /// whatever else it offers (the SDK's rule, in `ClientLogin::start`).
    fn scope(&self) -> SessionScope {
        match &self.recovery_phrase {
            None => SessionScope::Full,
            Some(_) => SessionScope::Recovery,
        }
    }

    fn into_factors(self) -> SignInFactors {
        let mut factors = match self.recovery_phrase {
            Some(phrase) => SignInFactors::recovery_phrase(phrase),
            None => SignInFactors::default(),
        };
        factors.password = self.password;
        factors.security_key = self.security_key;
        factors
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offered() -> Offered {
        Offered {
            password: Some(SecBuffer::from("pw")),
            security_key: None,
            recovery_phrase: None,
        }
    }

    fn phrase() -> RecoveryPhrase {
        let seed = citadel_sdk::prelude::RecoverySeed::from_bytes([7; 32]);
        RecoveryPhrase::new(seed, "acme.example").unwrap()
    }

    #[test]
    fn the_phrase_makes_a_recovery_session_and_nothing_else_does() {
        assert_eq!(offered().scope(), SessionScope::Full);
        let with_phrase = Offered {
            recovery_phrase: Some(phrase()),
            ..offered()
        };
        assert_eq!(with_phrase.scope(), SessionScope::Recovery);
    }

    #[test]
    fn the_phrase_is_carried_to_the_sdk() {
        let with_phrase = Offered {
            recovery_phrase: Some(phrase()),
            ..offered()
        };
        assert!(with_phrase.into_factors().recovery_phrase.is_some());
    }

    #[test]
    fn the_password_is_carried_through() {
        let factors = offered().into_factors();
        assert_eq!(factors.password.unwrap().as_ref(), b"pw");
        assert!(factors.security_key.is_none() && factors.recovery_phrase.is_none());
    }
}
