//! The supervisor's clock: tokio's monotonic time, counted from when the supervisor started.

use crate::kernel::supervisor::ports::Clock;
use crate::kernel::supervisor::types::Millis;
use futures::future::BoxFuture;
use futures::FutureExt;
use tokio::time::{sleep_until, Duration, Instant};

pub(super) struct TokioClock {
    start: Instant,
}

impl TokioClock {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl Clock for TokioClock {
    fn now(&self) -> Millis {
        Millis(u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX))
    }

    fn sleep_until(&self, at: Millis) -> BoxFuture<'static, ()> {
        sleep_until(self.start + Duration::from_millis(at.0)).boxed()
    }
}
