//! The lifecycle of one app instance: ONE state machine, held by the guest SDK and by every host
//! (the browser host reads the transition table from `bindings/AbiConstants.ts`, generated from
//! [`LifecycleState::after`], so neither side restates it).
//!
//! ```text
//! Created --initialised--> Initialised --start--> Running <--suspend/resume--> Suspended
//!                               |                    |                            |
//!                               +-------close--------+------------close-----------+
//!                                                    v
//!                                                 Closing --settled--> Closed
//! any state but Closed --abort--> Closed   (init failed, a trap, the watchdog, the grace expired)
//! ```
//!
//! - `Created`: the module is instantiated; `app_init` has not answered OK.
//! - `Initialised`: `app_init` answered OK; the app exists but has not been started.
//! - `Running`: the host delivered `AppEvent::Init` (the start); input and frames flow.
//! - `Suspended`: the surface is out of sight (`AppEvent::Suspend`); no frames, no input. Replies,
//!   storage changes and internal-HTTP frames still arrive: a suspended app is alive.
//! - `Closing`: the host called `app_close(reason)`; the app may still persist what it holds
//!   (the [`CLOSING_GROUPS`]) and receives the replies to those calls, for at most the install's
//!   `close_grace_ms`. Every other call answers `{unavailable: "closing"}`.
//! - `Closed`: nothing of the app runs again; any call answers `{unavailable: "closed"}`.
//!
//! A step the table does not list is refused ([`IllegalTransition`]): a second close, a resume
//! of a running app, anything after `Closed`.

use crate::abi::{REFUSED_CLOSED, REFUSED_CLOSING};
use crate::Group;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum LifecycleState {
    Created,
    Initialised,
    Running,
    Suspended,
    Closing,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum LifecycleStep {
    /// `app_init` answered OK.
    Initialised,
    /// The host delivered `AppEvent::Init`.
    Start,
    Suspend,
    Resume,
    /// The host called `app_close`.
    Close,
    /// The close ran and every reply it waited on was delivered.
    Settled,
    /// The instance ended without a settled close.
    Abort,
}

/// Why the host closes an app: the closed set `app_close(reason)` receives as [`Self::code`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[repr(i32)]
pub enum CloseReason {
    /// The surface went away: the zone or pane was unmounted, the person left the page.
    Unmount = 0,
    /// The install was removed.
    Uninstall = 1,
    /// The install no longer reaches this person here: they left its scope, disabled it for
    /// themselves, or a grant it runs on was withdrawn.
    AccessRevoked = 2,
    /// The page itself is going away (closed or reloaded).
    HostShutdown = 3,
    /// Another release of the app replaces this one.
    VersionUpdate = 4,
    /// The host must stop the app for a fault of its own (a port the app runs on failed).
    HostError = 5,
}

impl CloseReason {
    pub const ALL: [CloseReason; 6] = [
        Self::Unmount,
        Self::Uninstall,
        Self::AccessRevoked,
        Self::HostShutdown,
        Self::VersionUpdate,
        Self::HostError,
    ];

    pub const fn code(self) -> i32 {
        self as i32
    }

    pub fn from_code(code: i32) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.code() == code)
    }
}

/// Why the host suspends an app.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum SuspendReason {
    /// The app's surface is out of view (scrolled away, collapsed).
    Hidden,
    /// The whole page is in the background (another tab, minimised, kept for back/forward).
    Backgrounded,
}

/// The groups a closing app may still call: what it needs to persist its state, and its log.
pub const CLOSING_GROUPS: [Group; 2] = [Group::Storage, Group::Log];

/// A step the lifecycle does not allow from where it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IllegalTransition {
    pub from: LifecycleState,
    pub step: LifecycleStep,
}

impl LifecycleState {
    pub const ALL: [LifecycleState; 6] = [
        Self::Created,
        Self::Initialised,
        Self::Running,
        Self::Suspended,
        Self::Closing,
        Self::Closed,
    ];

    /// THE transition table: where `step` takes this state, or `None` when it is not allowed.
    pub const fn after(self, step: LifecycleStep) -> Option<LifecycleState> {
        use LifecycleState as S;
        use LifecycleStep as E;
        match (self, step) {
            (S::Closed, _) => None,
            (_, E::Abort) => Some(S::Closed),
            (S::Created, E::Initialised) => Some(S::Initialised),
            (S::Initialised, E::Start) => Some(S::Running),
            (S::Running, E::Suspend) => Some(S::Suspended),
            (S::Suspended, E::Resume) => Some(S::Running),
            (S::Initialised | S::Running | S::Suspended, E::Close) => Some(S::Closing),
            (S::Closing, E::Settled) => Some(S::Closed),
            _ => None,
        }
    }

    pub fn step(self, step: LifecycleStep) -> Result<LifecycleState, IllegalTransition> {
        self.after(step)
            .ok_or(IllegalTransition { from: self, step })
    }

    /// The refusal a host call in this state answers before anything else is looked at, or
    /// `None` when the call goes on to the usual checks.
    pub fn refusal_for(self, group: Group) -> Option<&'static str> {
        match self {
            Self::Closed => Some(REFUSED_CLOSED),
            Self::Closing if !CLOSING_GROUPS.contains(&group) => Some(REFUSED_CLOSING),
            _ => None,
        }
    }
}

#[cfg(feature = "ts")]
const STEPS: [LifecycleStep; 7] = [
    LifecycleStep::Initialised,
    LifecycleStep::Start,
    LifecycleStep::Suspend,
    LifecycleStep::Resume,
    LifecycleStep::Close,
    LifecycleStep::Settled,
    LifecycleStep::Abort,
];

/// Serde's snake_case tag of a unit variant, read back from its CBOR (one text string).
#[cfg(feature = "ts")]
fn tag<T: Serialize>(value: &T) -> std::string::String {
    crate::decode::<std::string::String>(&crate::encode(value))
        .expect("a unit variant encodes as its tag")
}

#[cfg(feature = "ts")]
pub(crate) fn render_ts(o: &mut std::string::String) {
    use std::fmt::Write;
    use std::string::String;
    use std::vec::Vec;
    writeln!(o, "\n/** THE lifecycle transition table (citadel-app-sdk-types lifecycle.rs): state -> step -> next. A step not listed is illegal. */").unwrap();
    writeln!(o, "export const LIFECYCLE_TRANSITIONS: Readonly<Record<LifecycleState, Readonly<Partial<Record<LifecycleStep, LifecycleState>>>>> = {{").unwrap();
    for state in LifecycleState::ALL {
        let steps: Vec<String> = STEPS
            .iter()
            .filter_map(|s| {
                state
                    .after(*s)
                    .map(|to| format!("{}: {:?}", tag(s), tag(&to)))
            })
            .collect();
        let body = match steps.is_empty() {
            true => String::from("{}"),
            false => format!("{{ {} }}", steps.join(", ")),
        };
        writeln!(o, "  {}: {body},", tag(&state)).unwrap();
    }
    writeln!(o, "}};\n\n/** `app_close`'s argument: the code of each CloseReason. */\nexport const CLOSE_REASON_CODE: Readonly<Record<CloseReason, number>> = {{").unwrap();
    for r in CloseReason::ALL {
        writeln!(o, "  {}: {},", tag(&r), r.code()).unwrap();
    }
    let groups: Vec<String> = CLOSING_GROUPS
        .iter()
        .map(|g| format!("{:?}", g.import_name()))
        .collect();
    writeln!(o, "}};\n\n/** The groups a closing app may still call. */\nexport const CLOSING_GROUPS = [{}] as const satisfies readonly Group[];", groups.join(", ")).unwrap();
}
