//! `InodeId` (`vfs-model.md` §2): 16 bytes, random or derived from a namespace and a key.

use crate::hash::{from_hex, lp, sha3_256, to_hex, TooLong};
use crate::volume::VolumeId;

pub const INODE_DOMAIN: &[u8] = b"citadel-vfs-inode-v1";

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InodeId(pub [u8; 16]);

impl core::fmt::Debug for InodeId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "InodeId({})", self.to_hex())
    }
}

/// The namespaces a derived id may come from (`vfs-model.md` §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Namespace {
    Root,
    Seed,
    AppHome,
    RevfsDir,
    RevfsFile,
    Transfer,
    ChatDoc,
    ChatFile,
}

impl Namespace {
    pub const ALL: [Self; 8] = [
        Self::Root,
        Self::Seed,
        Self::AppHome,
        Self::RevfsDir,
        Self::RevfsFile,
        Self::Transfer,
        Self::ChatDoc,
        Self::ChatFile,
    ];

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|n| n.as_str() == text)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::Seed => "seed",
            Self::AppHome => "app-home",
            Self::RevfsDir => "revfs-dir",
            Self::RevfsFile => "revfs-file",
            Self::Transfer => "transfer",
            Self::ChatDoc => "chat-doc",
            Self::ChatFile => "chat-file",
        }
    }
}

impl InodeId {
    /// From the caller's CSPRNG.
    pub const fn random(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// `SHA3-256("citadel-vfs-inode-v1" ‖ 0x00 ‖ lp(volume) ‖ lp(namespace) ‖ lp(key))[..16]`.
    pub fn derive(volume: &VolumeId, namespace: Namespace, key: &str) -> Result<Self, TooLong> {
        let mut framed = Vec::new();
        lp(&mut framed, &volume.to_string())?;
        lp(&mut framed, namespace.as_str())?;
        lp(&mut framed, key)?;
        let digest = sha3_256(&[INODE_DOMAIN, &[0], &framed]);
        let mut id = [0u8; 16];
        id.copy_from_slice(&digest[..16]);
        Ok(Self(id))
    }

    /// The volume's root directory.
    pub fn root(volume: &VolumeId) -> Self {
        Self::derive(volume, Namespace::Root, "")
            .unwrap_or_else(|_| unreachable!("a volume id is at most VOLUME_ID_MAX_BYTES"))
    }

    pub fn to_hex(&self) -> String {
        to_hex(&self.0)
    }

    /// 32 lowercase hex characters.
    pub fn from_hex(text: &str) -> Option<Self> {
        from_hex::<16>(text).map(Self)
    }
}

/// As 32 lowercase hex characters, the form every wire and fixture uses.
impl serde::Serialize for InodeId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> serde::Deserialize<'de> for InodeId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = <String as serde::Deserialize>::deserialize(d)?;
        Self::from_hex(&text).ok_or_else(|| serde::de::Error::custom("not 32 lowercase hex"))
    }
}
