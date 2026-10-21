//! The policy's requests-per-minute limit, per install per account.

use citadel_app_net_policy::MAX_REQUESTS_PER_MINUTE;
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};

const WINDOW_MS: u64 = 60_000;

/// Who a limit and a meter belong to: one install, used by one account.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct InstallKey {
    pub cid: u64,
    pub install_id: String,
}

/// A sliding window: at most `MAX_REQUESTS_PER_MINUTE` admitted in any 60 s. A refused
/// request is not recorded, so a caller that keeps asking is not locked out for longer.
#[derive(Default)]
pub(crate) struct RateLimiter {
    admitted: Mutex<HashMap<InstallKey, VecDeque<u64>>>,
}

impl RateLimiter {
    pub(crate) fn admit(&self, key: &InstallKey, now_ms: u64) -> bool {
        let mut admitted = self.admitted.lock();
        let times = admitted.entry(key.clone()).or_default();
        while times
            .front()
            .is_some_and(|&t| now_ms.saturating_sub(t) >= WINDOW_MS)
        {
            times.pop_front();
        }
        if times.len() >= MAX_REQUESTS_PER_MINUTE as usize {
            return false;
        }
        times.push_back(now_ms);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(install: &str) -> InstallKey {
        InstallKey {
            cid: 7,
            install_id: install.to_string(),
        }
    }

    #[test]
    fn the_sixty_first_request_in_a_minute_is_refused_and_the_window_slides() {
        let limiter = RateLimiter::default();
        for n in 0..MAX_REQUESTS_PER_MINUTE as u64 {
            assert!(limiter.admit(&key("a"), n), "request {n}");
        }
        assert!(!limiter.admit(&key("a"), 59_999));
        // Another install, and the same install for another account, have their own budgets.
        assert!(limiter.admit(&key("b"), 59_999));
        let other_account = InstallKey {
            cid: 8,
            install_id: "a".into(),
        };
        assert!(limiter.admit(&other_account, 59_999));
        // The first admission falls out of the window at 60 s.
        assert!(limiter.admit(&key("a"), 60_000));
        assert!(!limiter.admit(&key("a"), 60_000));
    }
}
