//! The collector's bounds and errors. Every bound is supplied by the caller: there is no
//! default ring size, because how much a session may hold depends on how often it is
//! uploaded, and that interval is configuration too.

use std::fmt;

/// How much one opted-in session's ring may hold before the oldest entries are evicted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingConfig {
    max_entries: usize,
    max_bytes: usize,
    max_message_bytes: usize,
}

impl RingConfig {
    /// `max_bytes` bounds the sum of every entry's message and target; `max_message_bytes`
    /// truncates each of the two, so one huge line cannot evict a whole ring and the
    /// largest possible entry (twice `max_message_bytes`) always fits.
    pub fn new(
        max_entries: usize,
        max_bytes: usize,
        max_message_bytes: usize,
    ) -> Result<Self, TelemetryError> {
        if max_entries == 0 {
            return Err(TelemetryError::InvalidConfig(
                "max_entries must be at least 1",
            ));
        }
        if max_message_bytes == 0 {
            return Err(TelemetryError::InvalidConfig(
                "max_message_bytes must be at least 1",
            ));
        }
        if max_message_bytes
            .checked_mul(2)
            .is_none_or(|largest| max_bytes < largest)
        {
            return Err(TelemetryError::InvalidConfig(
                "max_bytes must hold one maximal entry (2 * max_message_bytes)",
            ));
        }
        Ok(Self {
            max_entries,
            max_bytes,
            max_message_bytes,
        })
    }

    pub fn max_entries(&self) -> usize {
        self.max_entries
    }

    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    pub fn max_message_bytes(&self) -> usize {
        self.max_message_bytes
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelemetryError {
    InvalidConfig(&'static str),
    /// The cid is not a session this agent has registered with the collector.
    UnknownSession(u64),
    /// The session exists but has not opted in, so it has no ring to drain.
    NotOptedIn(u64),
    /// The OS could not supply randomness for a batch salt.
    Randomness(String),
}

impl fmt::Display for TelemetryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(why) => write!(f, "invalid telemetry config: {why}"),
            Self::UnknownSession(cid) => write!(f, "session {cid} is not registered"),
            Self::NotOptedIn(cid) => write!(f, "session {cid} has not opted in to telemetry"),
            Self::Randomness(err) => write!(f, "no randomness for the telemetry salt: {err}"),
        }
    }
}

impl std::error::Error for TelemetryError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bound_is_validated() {
        assert!(RingConfig::new(0, 10, 10).is_err());
        assert!(RingConfig::new(1, 10, 0).is_err());
        assert!(RingConfig::new(1, 19, 10).is_err());
        assert!(RingConfig::new(1, usize::MAX, usize::MAX).is_err());
        let ok = RingConfig::new(3, 100, 10).expect("valid");
        assert_eq!(
            (ok.max_entries(), ok.max_bytes(), ok.max_message_bytes()),
            (3, 100, 10)
        );
    }
}
