//! `SignedOp = { 1: header bstr, 2: body bstr, 3: key tstr(16), 4: sig bstr(3309) }`
//! (`vfs-signing.md` §1), at most `SIGNED_OP_MAX_BYTES`.

use crate::cbor::{bytes, decode, text, CodecError, MapReader, Value};
use crate::limits::SIGNED_OP_MAX_BYTES;
use crate::op::{verify_op, Op, OpError};
use crate::principal::is_key_fp;
use citadel_identity::{MlDsa65Pk, SignatureError, SIGNATURE_LEN};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedOp {
    pub header: Vec<u8>,
    /// Empty once redacted.
    pub body: Vec<u8>,
    /// The signing key's fingerprint.
    pub key: String,
    pub sig: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignedOpError {
    Codec(CodecError),
    Op(OpError),
    Signature(SignatureError),
}

impl SignedOp {
    pub fn encode(&self) -> Vec<u8> {
        Value::Map(vec![
            (1, Value::bytes(&self.header)),
            (2, Value::bytes(&self.body)),
            (3, Value::text(&self.key)),
            (4, Value::bytes(&self.sig)),
        ])
        .encode()
    }

    pub fn decode(raw: &[u8]) -> Result<Self, CodecError> {
        if raw.len() > SIGNED_OP_MAX_BYTES {
            return Err(CodecError::TooLarge);
        }
        let mut map = MapReader::new(decode(raw)?, "signed op")?;
        let signed = Self {
            header: bytes(map.take(1)?, "header")?,
            body: bytes(map.take(2)?, "body")?,
            key: text(map.take(3)?, "key")?,
            sig: bytes(map.take(4)?, "sig")?,
        };
        map.finish()?;
        if !is_key_fp(&signed.key) {
            return Err(CodecError::OutOfRange("key"));
        }
        if signed.sig.len() != SIGNATURE_LEN {
            return Err(CodecError::OutOfRange("sig"));
        }
        Ok(signed)
    }

    /// The signature over the header bytes, checked before anything decodes them.
    pub fn verify(&self, pk: &MlDsa65Pk) -> Result<(), SignatureError> {
        if citadel_identity::fingerprint(pk) != self.key {
            return Err(SignatureError::MalformedKey(
                "not the key this op names".to_string(),
            ));
        }
        verify_op(pk, &self.header, &self.sig)
    }

    /// The decoded op, after `verify`.
    pub fn op(&self) -> Result<Op, OpError> {
        Op::decode(&self.header, &self.body)
    }

    /// The same op with its body dropped (`vfs-ops.md` §5).
    pub fn redacted(&self) -> Self {
        Self {
            body: Vec::new(),
            ..self.clone()
        }
    }
}
