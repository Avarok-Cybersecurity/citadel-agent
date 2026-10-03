//! When a verified update is installed (pure).
//!
//! Installing restarts the agent, and every session lives in its memory, so it signs every
//! account out. That happens when the user asks for it, having been told, or when there is
//! nobody signed in to lose anything.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// "Restart to update", from a window or the menu bar.
    UserAsked,
    /// A check finished, or the idle re-check came round.
    Idle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Install,
    Wait(&'static str),
}

pub struct Situation {
    pub trigger: Trigger,
    /// A release is downloaded, verified and installable from here.
    pub ready: bool,
    pub signed_in: usize,
    /// "Automatically install updates when no account is signed in".
    pub auto_install: bool,
    /// Installing this version failed before; only the user may try it again.
    pub failed_before: bool,
}

pub fn decide(s: &Situation) -> Decision {
    if !s.ready {
        return Decision::Wait("no update is ready to install");
    }
    match s.trigger {
        Trigger::UserAsked => Decision::Install,
        Trigger::Idle if !s.auto_install => Decision::Wait("automatic installs are off"),
        Trigger::Idle if s.signed_in > 0 => Decision::Wait("an account is signed in"),
        Trigger::Idle if s.failed_before => Decision::Wait("installing this version failed before"),
        Trigger::Idle => Decision::Install,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(trigger: Trigger, ready: bool, signed_in: usize, auto: bool, failed: bool) -> Situation {
        Situation {
            trigger,
            ready,
            signed_in,
            auto_install: auto,
            failed_before: failed,
        }
    }

    #[test]
    fn nothing_is_installed_that_is_not_ready() {
        assert!(matches!(
            decide(&s(Trigger::UserAsked, false, 0, true, false)),
            Decision::Wait(_)
        ));
        assert!(matches!(
            decide(&s(Trigger::Idle, false, 0, true, false)),
            Decision::Wait(_)
        ));
    }

    #[test]
    fn the_user_may_install_with_accounts_open_and_after_a_failure() {
        assert_eq!(
            decide(&s(Trigger::UserAsked, true, 3, false, true)),
            Decision::Install
        );
    }

    #[test]
    fn automatic_installs_wait_for_nobody_signed_in() {
        assert_eq!(
            decide(&s(Trigger::Idle, true, 0, true, false)),
            Decision::Install
        );
        assert!(matches!(
            decide(&s(Trigger::Idle, true, 1, true, false)),
            Decision::Wait(_)
        ));
    }

    #[test]
    fn automatic_installs_respect_the_setting_and_never_retry_a_failure() {
        assert!(matches!(
            decide(&s(Trigger::Idle, true, 0, false, false)),
            Decision::Wait(_)
        ));
        assert!(matches!(
            decide(&s(Trigger::Idle, true, 0, true, true)),
            Decision::Wait(_)
        ));
    }
}
