//! Protocol ceilings and named values (`vfs-model.md` §5, §7). No value here is implied by the
//! reducer: an op always carries its mode, and every `VolumeLimits` is built with every field.

pub const NAME_MAX_BYTES: usize = 255;
/// The root is depth 0.
pub const DEPTH_MAX: u8 = 64;
pub const SIGNED_OP_MAX_BYTES: usize = 12_288;
pub const MIME_MAX_BYTES: usize = 127;
pub const VIRTUAL_PATH_MAX_BYTES: usize = 512;
pub const DOC_ID_MAX_BYTES: usize = 128;
pub const VOLUME_ID_MAX_BYTES: usize = 192;
/// A link's `max_claims` is 1..=this (`MAX_BOOKING_GUESTS`).
pub const MAX_LINK_CLAIMS: u64 = 50;
/// A link's admission lasts at most this long after its `Admit` (`SECURE_LINK_MAX_TTL_MS`).
pub const SECURE_LINK_MAX_TTL_MS: u64 = 30 * DAY_MS;

pub const ROOT_MODE: u16 = 0o770;
/// What a client offers when the person states no mode.
pub const DEFAULT_DIR_MODE: u16 = 0o770;
pub const DEFAULT_FILE_MODE: u16 = 0o660;
pub const APP_HOME_MODE: u16 = 0o770;

const DAY_MS: u64 = 24 * 60 * 60 * 1000;
const GIB: u64 = 1 << 30;

/// One volume's limits. Every field is required wherever it is built; there is no default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VolumeLimits {
    /// Live plus removed entries.
    pub max_entries: u32,
    pub max_entries_per_dir: u32,
    /// At most `DEPTH_MAX`.
    pub max_depth: u8,
    pub max_log_ops: u64,
    /// One file.
    pub max_blob_bytes: u64,
}

/// The limits every replica of a volume without a kernel shares (Pair, Device, Group).
pub const PEER_VOLUME_LIMITS: VolumeLimits = VolumeLimits {
    max_entries: 65_536,
    max_entries_per_dir: 4_096,
    max_depth: 64,
    max_log_ops: 1_048_576,
    max_blob_bytes: 4 * GIB,
};

/// Retention before purge in volumes without a kernel; nothing purges there automatically.
pub const PEER_RETENTION_MS: u64 = 30 * DAY_MS;

/// The rate window of an admission's `max_ops_per_min`.
pub const RATE_WINDOW_MS: u64 = 60_000;
