//! How often `IdentitySignChallenge` may run (`sign-in-with-citadel.md` §8): one request per
//! `min_interval` per connection, and after `wrong_password_limit` wrong passwords in a row
//! every request is `RateLimited` for `lockout`. Pure: the caller passes the time.
//!
//! The wrong-password count is the agent's, not a connection's: a page that reconnects must
//! not get five fresh guesses. The interval is per connection, so one window's sign-in does
//! not hold up another's.

use std::collections::HashMap;
use std::time::Duration;
use tokio::time::Instant;
use uuid::Uuid;

/// How often signing may run. The agent runs at `PROTOCOL`; a test may state another rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignLimits {
    /// The least time between two requests on one connection.
    pub min_interval: Duration,
    /// Wrong passwords in a row that start the lockout.
    pub wrong_password_limit: u32,
    /// How long the lockout refuses every request.
    pub lockout: Duration,
}

impl SignLimits {
    /// The protocol's rate (`citadel_identity::SIGN_*`, contract identity-api.md §1).
    pub const PROTOCOL: Self = Self {
        min_interval: Duration::from_secs(citadel_identity::SIGN_MIN_INTERVAL_S),
        wrong_password_limit: citadel_identity::SIGN_WRONG_PASSWORD_LIMIT,
        lockout: Duration::from_secs(citadel_identity::SIGN_LOCKOUT_S),
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    TooSoon { wait: Duration },
    LockedOut { wait: Duration },
}

pub(crate) struct SignGate {
    limits: SignLimits,
    last: HashMap<Uuid, Instant>,
    wrong_in_a_row: u32,
    locked_until: Option<Instant>,
}

impl SignGate {
    pub(crate) fn new(limits: SignLimits) -> Self {
        Self {
            limits,
            last: HashMap::new(),
            wrong_in_a_row: 0,
            locked_until: None,
        }
    }

    /// Admits a request on `conn` at `now` and counts it toward the interval, or refuses it.
    pub(crate) fn admit(&mut self, conn: Uuid, now: Instant) -> Result<(), Refusal> {
        if let Some(until) = self.locked_until.filter(|until| now < *until) {
            return Err(Refusal::LockedOut { wait: until - now });
        }
        let interval = self.limits.min_interval;
        // Entries past the interval decide nothing: dropping them bounds the map.
        self.last.retain(|_, at| now.duration_since(*at) < interval);
        if let Some(at) = self.last.get(&conn) {
            return Err(Refusal::TooSoon {
                wait: interval - now.duration_since(*at),
            });
        }
        self.last.insert(conn, now);
        Ok(())
    }

    pub(crate) fn wrong_password(&mut self, now: Instant) {
        self.wrong_in_a_row += 1;
        if self.wrong_in_a_row >= self.limits.wrong_password_limit {
            self.wrong_in_a_row = 0;
            self.locked_until = Some(now + self.limits.lockout);
        }
    }

    pub(crate) fn right_password(&mut self) {
        self.wrong_in_a_row = 0;
    }
}

#[cfg(test)]
mod tests {
    //! Control: drop the `locked_until` check in `admit` → the sixth request inside the
    //! lockout is admitted and `five_wrong_passwords_lock_every_connection_out` goes red.
    use super::*;

    const SHIPPED: SignLimits = SignLimits {
        min_interval: Duration::from_secs(2),
        wrong_password_limit: 5,
        lockout: Duration::from_secs(60),
    };

    #[test]
    fn one_request_per_interval_per_connection() {
        let (mut gate, t0) = (SignGate::new(SHIPPED), Instant::now());
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        assert_eq!(gate.admit(a, t0), Ok(()));
        let soon = t0 + Duration::from_millis(1500);
        assert_eq!(
            gate.admit(a, soon),
            Err(Refusal::TooSoon {
                wait: Duration::from_millis(500)
            })
        );
        assert_eq!(
            gate.admit(b, soon),
            Ok(()),
            "another connection is not held up"
        );
        assert_eq!(gate.admit(a, t0 + Duration::from_secs(2)), Ok(()));
    }

    #[test]
    fn five_wrong_passwords_lock_every_connection_out() {
        let (mut gate, t0) = (SignGate::new(SHIPPED), Instant::now());
        let mut now = t0;
        for _ in 0..5 {
            now += Duration::from_secs(3);
            // A fresh connection each time: reconnecting does not reset the count.
            assert_eq!(gate.admit(Uuid::new_v4(), now), Ok(()));
            gate.wrong_password(now);
        }
        let sixth = now + Duration::from_secs(3);
        assert_eq!(
            gate.admit(Uuid::new_v4(), sixth),
            Err(Refusal::LockedOut {
                wait: Duration::from_secs(57)
            })
        );
        assert_eq!(gate.admit(Uuid::new_v4(), now + SHIPPED.lockout), Ok(()));
    }

    #[test]
    fn a_right_password_resets_the_count() {
        let (mut gate, t0) = (SignGate::new(SHIPPED), Instant::now());
        for _ in 0..4 {
            gate.wrong_password(t0);
        }
        gate.right_password();
        gate.wrong_password(t0);
        assert_eq!(
            gate.admit(Uuid::new_v4(), t0),
            Ok(()),
            "four, then one: no lockout"
        );
    }
}
