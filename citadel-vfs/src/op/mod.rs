//! Ops: kinds, header, body, the signed envelope, structural validity (`vfs-ops.md` §1–§3).

mod body;
mod header;
mod kind;
mod signed;
mod structure;
mod verify;

pub use body::{Body, Lasts};
pub use header::{Header, HeaderError, Stamp, HEADER_VERSION, LAMPORT_MAX};
pub use kind::OpKind;
pub use signed::{SignedOp, SignedOpError};
pub use structure::{check_alone, check_chain, Invalid};
pub use verify::{
    claim_message, op_message, verify_claim, verify_op, VFS_CLAIM_DOMAIN, VFS_OP_DOMAIN,
};

use crate::cbor::CodecError;
use crate::hash::sha3_256;
use crate::inode::InodeId;
use crate::principal::Principal;
use crate::volume::VolumeId;

/// A decoded op: its header, its body (`None` once redacted, `vfs-ops.md` §5) and `op_hash`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Op {
    pub header: Header,
    pub body: Option<Body>,
    /// `SHA3-256(header bytes)`.
    pub op_hash: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpError {
    Header(HeaderError),
    Body(CodecError),
    /// The body does not hash to the header's `body_hash`.
    BodyHash,
}

/// `SHA3-256(header bytes)`.
pub fn op_hash(header: &[u8]) -> [u8; 32] {
    sha3_256(&[header])
}

impl Op {
    /// From the bytes a `SignedOp` carries. An empty body is a redacted op.
    pub fn decode(header: &[u8], body: &[u8]) -> Result<Self, OpError> {
        let decoded = Header::decode(header).map_err(OpError::Header)?;
        let body = if body.is_empty() {
            None
        } else {
            if sha3_256(&[body]) != decoded.body_hash {
                return Err(OpError::BodyHash);
            }
            Some(Body::decode(decoded.kind, body).map_err(OpError::Body)?)
        };
        Ok(Self {
            header: decoded,
            body,
            op_hash: op_hash(header),
        })
    }
}

/// An op before it is stamped into bytes: what a client, a kernel or a migration pass writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub volume: VolumeId,
    pub op_id: [u8; 16],
    pub author: Principal,
    pub lamport: u64,
    pub prev: Option<[u8; 32]>,
    pub target: InodeId,
    pub at_ms: u64,
    pub body: Body,
}

impl Draft {
    pub fn header(&self) -> Header {
        Header {
            volume: self.volume.to_string(),
            op_id: self.op_id,
            author: self.author.clone(),
            lamport: self.lamport,
            prev: self.prev,
            kind: self.body.kind(),
            target: self.target,
            body_hash: sha3_256(&[&self.body.encode()]),
            at_ms: self.at_ms,
        }
    }

    /// The header and body bytes a signer signs and a log carries.
    pub fn to_bytes(&self) -> (Vec<u8>, Vec<u8>) {
        (self.header().encode(), self.body.encode())
    }

    pub fn to_op(&self) -> Op {
        let header = self.header();
        let op_hash = op_hash(&header.encode());
        Op {
            header,
            body: Some(self.body.clone()),
            op_hash,
        }
    }
}
