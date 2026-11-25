//! The eleven op kinds (`vfs-ops.md` §1); the numbers are frozen.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OpKind {
    Create = 1,
    Place = 2,
    SetPointee = 3,
    Remove = 4,
    Restore = 5,
    Purge = 6,
    Chmod = 7,
    Chown = 8,
    Admit = 9,
    Dismiss = 10,
    Claim = 11,
}

impl OpKind {
    pub const ALL: [OpKind; 11] = [
        Self::Create,
        Self::Place,
        Self::SetPointee,
        Self::Remove,
        Self::Restore,
        Self::Purge,
        Self::Chmod,
        Self::Chown,
        Self::Admit,
        Self::Dismiss,
        Self::Claim,
    ];

    pub fn from_code(code: u64) -> Option<Self> {
        Self::ALL.into_iter().find(|k| *k as u64 == code)
    }

    pub fn code(self) -> u64 {
        self as u64
    }

    /// The ✎ ops of `vfs-authorize.md` §4: they need `role.upload` in Node and Personal volumes.
    pub fn needs_upload(self) -> bool {
        matches!(
            self,
            Self::Create
                | Self::Place
                | Self::SetPointee
                | Self::Remove
                | Self::Restore
                | Self::Purge
        )
    }

    /// The kinds an outsider may author (`vfs-authorize.md` §4.0).
    pub fn outsider_may(self) -> bool {
        matches!(
            self,
            Self::Create | Self::Place | Self::SetPointee | Self::Remove | Self::Claim
        )
    }
}
