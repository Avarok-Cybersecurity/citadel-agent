//! The policy's requests-per-minute limit, per app per account.
//!
//! Keyed by the app id, not the install id: both come from the page, but an install id is
//! minted per install and could be rotated at will to reset the budget, while the app ids one
//! account may spend from are capped at `MAX_APPS_PER_ACCOUNT` live windows. A page rotating
//! app ids therefore reaches at most that many budgets, not an unbounded number.

use citadel_app_net_policy::MAX_REQUESTS_PER_MINUTE;
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};

const WINDOW_MS: u64 = 60_000;
/// The most apps one account may have requests in flight for within one window.
pub(crate) const MAX_APPS_PER_ACCOUNT: usize = 16;

/// Who a meter belongs to: one install, used by one account.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct InstallKey {
    pub cid: u64,
    pub install_id: String,
}

/// A sliding window per (account, app): at most `MAX_REQUESTS_PER_MINUTE` admitted in any
/// 60 s. A refused request is not recorded, so a caller that keeps asking is not locked out
/// for longer. An app whose window has emptied is forgotten, so the per-account app cap
/// counts only apps that spent within the last minute, and the map never outgrows it.
#[derive(Default)]
pub(crate) struct RateLimiter {
    admitted: Mutex<HashMap<u64, HashMap<String, VecDeque<u64>>>>,
}

impl RateLimiter {
    pub(crate) fn admit(&self, cid: u64, app_id: &str, now_ms: u64) -> bool {
        let mut admitted = self.admitted.lock();
        let apps = admitted.entry(cid).or_default();
        apps.retain(|_, times| {
            while times
                .front()
                .is_some_and(|&t| now_ms.saturating_sub(t) >= WINDOW_MS)
            {
                times.pop_front();
            }
            !times.is_empty()
        });
        if !apps.contains_key(app_id) && apps.len() >= MAX_APPS_PER_ACCOUNT {
            return false;
        }
        let times = apps.entry(app_id.to_string()).or_default();
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

    #[test]
    fn the_sixty_first_request_in_a_minute_is_refused_and_the_window_slides() {
        let limiter = RateLimiter::default();
        for n in 0..MAX_REQUESTS_PER_MINUTE as u64 {
            assert!(limiter.admit(7, "a", n), "request {n}");
        }
        assert!(!limiter.admit(7, "a", 59_999));
        // Another app, and the same app for another account, have their own budgets.
        assert!(limiter.admit(7, "b", 59_999));
        assert!(limiter.admit(8, "a", 59_999));
        // The first admission falls out of the window at 60 s.
        assert!(limiter.admit(7, "a", 60_000));
        assert!(!limiter.admit(7, "a", 60_000));
    }

    #[test]
    fn a_seventeenth_app_is_refused_until_one_falls_out_of_the_window() {
        let limiter = RateLimiter::default();
        for app in 0..MAX_APPS_PER_ACCOUNT {
            assert!(limiter.admit(7, &format!("app{app}"), 0), "app {app}");
        }
        assert!(!limiter.admit(7, "app16", 1));
        // The control: an app already counted still spends, and another account is apart.
        assert!(limiter.admit(7, "app0", 1));
        assert!(limiter.admit(8, "app16", 1));
        // Once the first sixteen's windows empty, the seventeenth gets one.
        assert!(limiter.admit(7, "app16", 60_001));
    }
}
