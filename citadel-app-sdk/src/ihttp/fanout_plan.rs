//! The fan-out's bookkeeping, pure: which targets run, which wait, and what each came to, under
//! a concurrency cap, a per-call and a total time budget, and a byte budget over all responses.
//! Time is what the caller says has passed (`elapse`); nothing here reads a clock.

use super::Response;
use citadel_app_sdk_types::Refused;
use std::collections::{BTreeMap, VecDeque};

/// The bounds of one fan-out. Every field is required, and a zero one admits nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FanoutLimits {
    pub max_concurrency: u32,
    pub per_call_ms: u32,
    pub budget_ms: u32,
    pub max_total_bytes: u64,
}

/// What one target came to.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Ok(Response),
    Failed(Refused),
    /// Never asked, or stopped, because the budget ran out or the fan-out was cancelled.
    Skipped,
}

/// `Refused::Limit` of a call that ran past `per_call_ms`.
pub const LIMIT_PER_CALL_MS: &str = "fanout_per_call_ms";
/// `Refused::Limit` of the call whose response took the total past `max_total_bytes`.
pub const LIMIT_TOTAL_BYTES: &str = "fanout_bytes";
/// `Refused::Policy` of every target when a limit is zero.
pub const POLICY_ZERO_LIMIT: &str = "fanout_zero_limit";

pub(crate) struct Plan {
    limits: FanoutLimits,
    waiting: VecDeque<usize>,
    /// Running targets, with how long each has run.
    running: BTreeMap<usize, u32>,
    settled: Vec<Option<Outcome>>,
    elapsed_ms: u32,
    bytes: u64,
}

impl Plan {
    pub fn new(targets: usize, limits: FanoutLimits) -> Plan {
        let zero = limits.max_concurrency == 0
            || limits.per_call_ms == 0
            || limits.budget_ms == 0
            || limits.max_total_bytes == 0;
        let refused = || Some(Outcome::Failed(Refused::Policy(POLICY_ZERO_LIMIT.into())));
        Plan {
            limits,
            waiting: if zero {
                VecDeque::new()
            } else {
                (0..targets).collect()
            },
            running: BTreeMap::new(),
            settled: (0..targets)
                .map(|_| if zero { refused() } else { None })
                .collect(),
            elapsed_ms: 0,
            bytes: 0,
        }
    }

    /// The targets to start now: as many waiting ones as the cap leaves room for.
    pub fn admit(&mut self) -> Vec<usize> {
        let mut started = Vec::new();
        while self.running.len() < self.limits.max_concurrency as usize {
            let Some(i) = self.waiting.pop_front() else {
                break;
            };
            self.running.insert(i, 0);
            started.push(i);
        }
        started
    }

    /// `n` more response bytes arrived for `i`: the refusal to stop `i` with, if that takes the
    /// total past the budget.
    pub fn bytes(&mut self, i: usize, n: usize) -> Option<Refused> {
        self.bytes = self.bytes.saturating_add(n as u64);
        let over = self.bytes > self.limits.max_total_bytes && self.running.contains_key(&i);
        over.then(|| Refused::Limit(LIMIT_TOTAL_BYTES.into()))
    }

    /// Record what `i` came to, once: false when it was already settled (a stopped call's own
    /// end arriving later).
    pub fn settle(&mut self, i: usize, outcome: Outcome) -> bool {
        let Some(slot) = self.settled.get_mut(i) else {
            return false;
        };
        if slot.is_some() {
            return false;
        }
        *slot = Some(outcome);
        self.running.remove(&i);
        self.waiting.retain(|w| *w != i);
        true
    }

    /// `ms` passed: the running targets to stop and what each came to. Past the budget,
    /// everything still running is skipped, and so is everything waiting.
    pub fn elapse(&mut self, ms: u32) -> Vec<(usize, Outcome)> {
        self.elapsed_ms = self.elapsed_ms.saturating_add(ms);
        if self.elapsed_ms >= self.limits.budget_ms {
            return self.stop_all();
        }
        let per_call = self.limits.per_call_ms;
        let mut late = Vec::new();
        for (i, ran) in self.running.iter_mut() {
            *ran = ran.saturating_add(ms);
            if *ran >= per_call {
                late.push(*i);
            }
        }
        let refused = || Outcome::Failed(Refused::Limit(LIMIT_PER_CALL_MS.into()));
        late.into_iter().map(|i| (i, refused())).collect()
    }

    /// Everything running is to stop, skipped; everything waiting is settled skipped.
    pub fn stop_all(&mut self) -> Vec<(usize, Outcome)> {
        for i in std::mem::take(&mut self.waiting) {
            self.settled[i] = Some(Outcome::Skipped);
        }
        self.running
            .keys()
            .map(|i| (*i, Outcome::Skipped))
            .collect()
    }

    pub fn is_done(&self) -> bool {
        self.settled.iter().all(Option::is_some)
    }

    /// Every target's outcome, in target order, once `is_done`.
    pub fn outcomes(&mut self) -> Vec<Outcome> {
        self.settled
            .iter_mut()
            .map(|o| o.take().unwrap_or(Outcome::Skipped))
            .collect()
    }
}

#[cfg(test)]
#[path = "fanout_plan_tests.rs"]
mod tests;
