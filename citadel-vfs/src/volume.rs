//! Volume ids and kinds (`vfs-model.md` §1). An id is derived from what the volume is, never chosen.

use crate::limits::VOLUME_ID_MAX_BYTES;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum VolumeKind {
    Personal,
    Pair,
    Node,
    Device,
    Group,
}

/// How positions are given (`vfs-ops.md` R1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeMode {
    /// One sequence: the kernel's `seq` (Personal, Node) or the group's `(seq, index)`.
    Ordered,
    /// No order between replicas: the stamp orders (Pair, Device).
    Merge,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum VolumeId {
    Personal {
        cid: u64,
    },
    /// `lo < hi`, always.
    Pair {
        lo: u64,
        hi: u64,
    },
    /// 1..=128 of `[A-Za-z0-9_-]`.
    Node {
        node_id: String,
    },
    Device {
        cid: u64,
    },
    Group {
        creator: u64,
        mgid: u128,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Malformed;

impl VolumeId {
    pub fn parse(text: &str) -> Result<Self, Malformed> {
        if text.len() > VOLUME_ID_MAX_BYTES {
            return Err(Malformed);
        }
        let (prefix, rest) = text.split_once(':').ok_or(Malformed)?;
        match prefix {
            "personal" => Ok(Self::Personal {
                cid: decimal(rest)?,
            }),
            "device" => Ok(Self::Device {
                cid: decimal(rest)?,
            }),
            "pair" => {
                let (lo, hi) = rest.split_once(':').ok_or(Malformed)?;
                let (lo, hi) = (decimal(lo)?, decimal(hi)?);
                if lo < hi {
                    Ok(Self::Pair { lo, hi })
                } else {
                    Err(Malformed)
                }
            }
            "group" => {
                let (creator, mgid) = rest.split_once(':').ok_or(Malformed)?;
                Ok(Self::Group {
                    creator: decimal(creator)?,
                    mgid: decimal(mgid)?,
                })
            }
            "node" if is_node_id(rest) => Ok(Self::Node {
                node_id: rest.to_string(),
            }),
            _ => Err(Malformed),
        }
    }

    /// The pair of two distinct accounts, in either order; `None` for one account with itself.
    pub fn pair(a: u64, b: u64) -> Option<Self> {
        match a.cmp(&b) {
            core::cmp::Ordering::Less => Some(Self::Pair { lo: a, hi: b }),
            core::cmp::Ordering::Greater => Some(Self::Pair { lo: b, hi: a }),
            core::cmp::Ordering::Equal => None,
        }
    }

    pub fn node(node_id: &str) -> Result<Self, Malformed> {
        Self::parse(&format!("node:{node_id}"))
    }

    pub fn kind(&self) -> VolumeKind {
        match self {
            Self::Personal { .. } => VolumeKind::Personal,
            Self::Pair { .. } => VolumeKind::Pair,
            Self::Node { .. } => VolumeKind::Node,
            Self::Device { .. } => VolumeKind::Device,
            Self::Group { .. } => VolumeKind::Group,
        }
    }

    pub fn mode(&self) -> VolumeMode {
        match self.kind() {
            VolumeKind::Pair | VolumeKind::Device => VolumeMode::Merge,
            VolumeKind::Personal | VolumeKind::Node | VolumeKind::Group => VolumeMode::Ordered,
        }
    }

    /// Whether the volume has a kernel (its limits are admission checks, not reducer rules).
    pub fn has_kernel(&self) -> bool {
        matches!(self.kind(), VolumeKind::Personal | VolumeKind::Node)
    }
}

impl core::fmt::Display for VolumeId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Personal { cid } => write!(f, "personal:{cid}"),
            Self::Pair { lo, hi } => write!(f, "pair:{lo}:{hi}"),
            Self::Node { node_id } => write!(f, "node:{node_id}"),
            Self::Device { cid } => write!(f, "device:{cid}"),
            Self::Group { creator, mgid } => write!(f, "group:{creator}:{mgid}"),
        }
    }
}

fn is_node_id(s: &str) -> bool {
    (1..=128).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Decimal without leading zeros ("0" itself is one digit).
fn decimal<T: core::str::FromStr>(s: &str) -> Result<T, Malformed> {
    let digits = !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits || (s.len() > 1 && s.starts_with('0')) {
        return Err(Malformed);
    }
    s.parse().map_err(|_| Malformed)
}
