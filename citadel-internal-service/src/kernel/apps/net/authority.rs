//! The workspace server's word on what an install may reach, as a port.
//!
//! The grant a page sends with `AppFetch` is the page's request, nothing more: a page can say
//! anything. The server is the authority on what the install was approved for and whether this
//! member may use it, so the agent asks it (`AppsRequest::HostNetGrant`) over the session's own
//! link and reads the answer off that link (kernel/c2s_reader.rs).
//!
//! The wire format belongs to the workspace protocol, which this crate does not depend on, so
//! the binary that ships supplies it, as it supplies the telemetry codec.

/// What the server says one install may reach, for the member who asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelGrant {
    /// The app the install is of. A request naming another app is refused.
    pub app_id: String,
    /// The hosts the install was approved for. A page may ask for fewer, never more.
    pub hosts: Vec<String>,
    /// Whether the install may reach private addresses: its installer still holds the right
    /// to allow it. The member's own grant on this agent is needed as well.
    pub private: bool,
    /// The manifest's `servers` the install was approved `citadel.session` to, as canonical
    /// domains (kernel/apps/session). Empty when it was not granted the capability.
    pub servers: Vec<String>,
    /// The manifest's `limits.session`: present exactly when `citadel.session` was granted.
    pub session: Option<SessionLimits>,
}

/// An install's `limits.session` (`citadel-session-abi.md` §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionLimits {
    /// `1..=1_048_576`; the agent also caps every body at `APP_SESSION_MAX_BODY_BYTES`.
    pub max_msg_bytes: u32,
    /// `1..=64`; enforced by the host, which meters the guest.
    pub max_msgs_per_s: u32,
}

/// What a frame the server sent a session means for host egress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthoritySignal {
    /// Not about host egress: it goes on to the session's windows.
    Unrelated,
    /// The answer to a question tagged `tag`. `None` is the server's refusal, the same for
    /// every reason it has.
    Grant {
        install_id: String,
        tag: u64,
        grant: Option<KernelGrant>,
    },
    /// The member's installs changed: what the server said about them may no longer hold.
    InstallsChanged,
}

/// The workspace protocol's encoding of the question and its answers.
pub trait HostNetAuthority: Send + Sync + 'static {
    /// The request asking what `install_id` may reach; the server echoes `tag`.
    fn grant_query(&self, install_id: &str, tag: u64) -> Result<Vec<u8>, String>;
    fn classify(&self, frame: &[u8]) -> AuthoritySignal;
}
