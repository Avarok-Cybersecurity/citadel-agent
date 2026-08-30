//! When a connection attempt may proceed, and when its ending is worth
//! reporting.
//!
//! Pure decisions, separated from the WASM plumbing so they can be tested
//! without a browser. Both were previously inline in `init_inner` and in the
//! communication task, and both were wrong in ways that only showed up on the
//! unhappy path.

/// Whether an initialisation attempt must be refused outright.
///
/// Only a non-restart `init()` against a live connection: calling it twice is a
/// caller error, and the message points at `restart()`.
///
/// A restart is never refused, and specifically not when nothing is
/// initialised. `restart()` used to require `is_initialized()` and error
/// otherwise, but it tears the old connection down BEFORE it connects — so a
/// restart whose connect then failed left the state destroyed and the flag
/// false. The UI's "Retry Now" button calls only `restart()`, so the second
/// attempt got "Not initialized. Call init() first." and every attempt after it
/// did too: **one failed retry disabled retrying**, permanently, for the life of
/// the page. A restart means "get me connected"; with nothing to tear down there
/// is simply less to do.
pub fn refuse_init(restart: bool, initialized: bool) -> bool {
    !restart && initialized
}

/// Whether the existing connection must be torn down before connecting.
pub fn teardown_before_connect(restart: bool, initialized: bool) -> bool {
    restart && initialized
}

/// Whether a communication task that has just ended should tell JavaScript the
/// connection died.
///
/// Only if it is still the CURRENT connection's task. `close_connection` drops
/// the state, which ends the task, which called `on_websocket_disconnected` —
/// so every deliberate teardown looked exactly like a failure: the retry modal
/// reappeared during a restart the user had just asked for, and background
/// services were stopped on a clean sign-out.
///
/// Generations distinguish the two without a "deliberate" flag that a race can
/// read at the wrong moment: a task reports only while the generation it was
/// spawned under is still the live one.
pub fn should_report_death(task_generation: u64, current_generation: u64) -> bool {
    task_generation == current_generation
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_init_is_refused_but_a_restart_is_not() {
        assert!(refuse_init(false, true), "init() twice is a caller error");
        assert!(
            !refuse_init(true, true),
            "restart() over a live connection is the point of it"
        );
        assert!(!refuse_init(false, false), "the first init must proceed");
        assert!(
            !refuse_init(true, false),
            "a restart with nothing to replace must proceed"
        );
    }

    #[test]
    fn only_a_restart_over_a_live_connection_tears_down() {
        assert!(teardown_before_connect(true, true));
        assert!(
            !teardown_before_connect(true, false),
            "nothing to tear down"
        );
        assert!(!teardown_before_connect(false, false));
        assert!(!teardown_before_connect(false, true));
    }

    #[test]
    fn a_failed_restart_does_not_disable_retrying() {
        // The exact sequence that bricked it: restart tears down, its connect
        // fails, so the flag is now false -- and the UI's only retry path is
        // restart(). If this is refused, the button is dead for the life of the
        // page.
        let initialized_after_failed_restart = false;
        assert!(
            !refuse_init(true, initialized_after_failed_restart),
            "a retry after a failed restart must be allowed to proceed"
        );
    }

    #[test]
    fn a_superseded_task_stays_quiet_and_the_live_one_does_not() {
        // A deliberate teardown bumps the generation, so the task it ended
        // belongs to a connection that is no longer current.
        assert!(
            !should_report_death(1, 2),
            "a torn-down connection is not a failure"
        );
        assert!(
            should_report_death(2, 2),
            "the live connection dying IS a failure"
        );
        // And a task from two restarts ago stays quiet too.
        assert!(!should_report_death(1, 3));
    }

    /// The decisions above are worth nothing unless `lib.rs` asks them.
    ///
    /// A pure function passes its own tests whether or not anything calls it,
    /// and both defects here were the call site rather than the rule.
    #[test]
    fn lib_actually_asks_these_questions() {
        let source = include_str!("lib.rs");
        let code: String = source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            !code.contains("Not initialized. Call init() first."),
            "restart() still refuses when nothing is initialised, which is the \
             state its own failed attempt leaves behind — one failed retry then \
             disables the retry button permanently."
        );
        assert!(
            code.contains("refuse_init(restart, initialized)"),
            "init_inner no longer asks refuse_init"
        );
        assert!(
            code.contains("teardown_before_connect(restart, initialized)"),
            "init_inner no longer asks teardown_before_connect"
        );

        // The death report must be guarded. Checking the guard exists is not
        // enough — it has to sit between the task ending and the callback.
        let report = code
            .find("on_websocket_disconnected(\"WebSocket communication task ended\")")
            .expect("the communication task no longer reports a death at all");
        let guard = code
            .find("should_report_death(")
            .expect("nothing consults should_report_death");
        assert!(
            guard < report,
            "the death callback is not behind should_report_death, so a \
             deliberate close still reports the connection as failed"
        );

        assert!(
            code.contains("CONNECTION_GENERATION.fetch_add"),
            "close_connection no longer bumps the generation, so a task it ends \
             still sees itself as current and reports a failure"
        );
    }
}
