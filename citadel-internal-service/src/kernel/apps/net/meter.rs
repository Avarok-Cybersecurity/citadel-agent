//! Bytes each install moved through the agent, per account; the page shows them.

use super::rate::InstallKey;
use parking_lot::Mutex;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Usage {
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub requests: u64,
    pub refused: u64,
}

#[derive(Default)]
pub(crate) struct Meter {
    usage: Mutex<HashMap<InstallKey, Usage>>,
}

impl Meter {
    fn with(&self, key: &InstallKey, change: impl FnOnce(&mut Usage)) {
        change(self.usage.lock().entry(key.clone()).or_default());
    }

    pub(crate) fn request(&self, key: &InstallKey) {
        self.with(key, |u| u.requests = u.requests.saturating_add(1));
    }

    pub(crate) fn refused(&self, key: &InstallKey) {
        self.with(key, |u| u.refused = u.refused.saturating_add(1));
    }

    pub(crate) fn sent(&self, key: &InstallKey, bytes: usize) {
        self.with(key, |u| {
            u.bytes_out = u.bytes_out.saturating_add(bytes as u64)
        });
    }

    pub(crate) fn received(&self, key: &InstallKey, bytes: usize) {
        self.with(key, |u| {
            u.bytes_in = u.bytes_in.saturating_add(bytes as u64)
        });
    }

    pub(crate) fn read(&self, key: &InstallKey) -> Usage {
        self.usage.lock().get(key).copied().unwrap_or_default()
    }
}
