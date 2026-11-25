//! The strict decoder: exactly one canonical item spanning all of the input.

use super::{CodecError, Value, ARRAY, BYTES, MAP, TEXT, UINT};

/// Deepest nesting any VFS shape has is 4 (a `Dismiss` body); 8 leaves room and bounds the stack.
const MAX_DEPTH: usize = 8;

pub fn decode(bytes: &[u8]) -> Result<Value, CodecError> {
    let mut reader = Reader { bytes, at: 0 };
    let value = reader.item(0)?;
    if reader.at != bytes.len() {
        return Err(CodecError::Trailing);
    }
    Ok(value)
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], CodecError> {
        let end = self.at.checked_add(n).ok_or(CodecError::Truncated)?;
        let slice = self.bytes.get(self.at..end).ok_or(CodecError::Truncated)?;
        self.at = end;
        Ok(slice)
    }

    fn head(&mut self) -> Result<(u8, u64), CodecError> {
        let first = self.take(1)?[0];
        let (major, info) = (first >> 5, first & 31);
        if info < 24 {
            return Ok((major, u64::from(info)));
        }
        let (width, floor) = match info {
            24 => (1, 24u64),
            25 => (2, 1 << 8),
            26 => (4, 1 << 16),
            27 => (8, 1 << 32),
            _ => return Err(CodecError::Indefinite),
        };
        let n = self
            .take(width)?
            .iter()
            .fold(0u64, |acc, b| (acc << 8) | u64::from(*b));
        if n < floor {
            return Err(CodecError::NonShortest);
        }
        Ok((major, n))
    }

    /// A length that cannot exceed what remains (so a hostile head allocates nothing).
    fn len(&self, n: u64) -> Result<usize, CodecError> {
        let n = usize::try_from(n).map_err(|_| CodecError::Truncated)?;
        if n > self.bytes.len() - self.at {
            return Err(CodecError::Truncated);
        }
        Ok(n)
    }

    fn item(&mut self, depth: usize) -> Result<Value, CodecError> {
        if depth > MAX_DEPTH {
            return Err(CodecError::TooDeep);
        }
        let (major, n) = self.head()?;
        match major {
            UINT => Ok(Value::Uint(n)),
            BYTES => {
                let n = self.len(n)?;
                Ok(Value::Bytes(self.take(n)?.to_vec()))
            }
            TEXT => {
                let n = self.len(n)?;
                let raw = self.take(n)?;
                let text = core::str::from_utf8(raw).map_err(|_| CodecError::BadUtf8)?;
                Ok(Value::Text(text.to_string()))
            }
            ARRAY => {
                // Every item takes at least one byte.
                let n = self.len(n)?;
                let mut items = Vec::with_capacity(n);
                for _ in 0..n {
                    items.push(self.item(depth + 1)?);
                }
                Ok(Value::Array(items))
            }
            MAP => {
                let n = self.len(n)?;
                let mut entries: Vec<(u64, Value)> = Vec::with_capacity(n);
                for _ in 0..n {
                    let (key_major, key) = self.head()?;
                    if key_major != UINT {
                        return Err(CodecError::KeyType);
                    }
                    if entries.last().is_some_and(|(prev, _)| *prev >= key) {
                        return Err(CodecError::Unsorted);
                    }
                    entries.push((key, self.item(depth + 1)?));
                }
                Ok(Value::Map(entries))
            }
            _ => Err(CodecError::Unsupported),
        }
    }
}
