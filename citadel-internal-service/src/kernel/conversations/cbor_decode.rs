//! Reading CBOR: anything cbor-x writes, each header's width kept so it
//! re-encodes to the same bytes; anything malformed an error, never a panic.

use super::{DecodeError, Value, Width};

const MAX_DEPTH: usize = 64;

impl Value {
    /// One complete value; trailing bytes are an error.
    pub(crate) fn decode(bytes: &[u8]) -> Result<Value, DecodeError> {
        let mut reader = Reader { bytes, at: 0 };
        let value = reader.value(0)?;
        if reader.at != bytes.len() {
            return Err(DecodeError(format!(
                "{} trailing byte(s) after the value",
                bytes.len() - reader.at
            )));
        }
        Ok(value)
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], DecodeError> {
        let end = self
            .at
            .checked_add(n)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| DecodeError(format!("truncated at byte {}", self.at)))?;
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    fn argument(&mut self, info: u8) -> Result<(u64, Width), DecodeError> {
        Ok(match info {
            0..=23 => (info as u64, Width::Inline),
            24 => (self.take(1)?[0] as u64, Width::One),
            25 => (
                u16::from_be_bytes(self.take(2)?.try_into().unwrap()) as u64,
                Width::Two,
            ),
            26 => (
                u32::from_be_bytes(self.take(4)?.try_into().unwrap()) as u64,
                Width::Four,
            ),
            27 => (
                u64::from_be_bytes(self.take(8)?.try_into().unwrap()),
                Width::Eight,
            ),
            _ => return Err(DecodeError(format!("indefinite or reserved length {info}"))),
        })
    }

    /// A length that cannot exceed what is left of the input (each element
    /// takes at least a byte), so a forged header cannot ask for a huge allocation.
    fn length(&mut self, info: u8) -> Result<(usize, Width), DecodeError> {
        let (n, w) = self.argument(info)?;
        let left = (self.bytes.len() - self.at) as u64;
        if n > left {
            return Err(DecodeError(format!(
                "length {n} exceeds the {left} bytes left"
            )));
        }
        Ok((n as usize, w))
    }

    fn value(&mut self, depth: usize) -> Result<Value, DecodeError> {
        if depth > MAX_DEPTH {
            return Err(DecodeError("nested too deeply".to_string()));
        }
        let initial = self.take(1)?[0];
        let (major, info) = (initial >> 5, initial & 0x1f);
        Ok(match major {
            0 => {
                let (n, w) = self.argument(info)?;
                Value::Uint(n, w)
            }
            1 => {
                let (n, w) = self.argument(info)?;
                Value::Nint(n, w)
            }
            2 => {
                let (n, _) = self.length(info)?;
                Value::Bytes(self.take(n)?.to_vec())
            }
            3 => {
                let (n, _) = self.length(info)?;
                let text = std::str::from_utf8(self.take(n)?)
                    .map_err(|err| DecodeError(format!("invalid UTF-8: {err}")))?;
                Value::Text(text.to_string())
            }
            4 => {
                let (n, _) = self.length(info)?;
                let mut items = Vec::with_capacity(n);
                for _ in 0..n {
                    items.push(self.value(depth + 1)?);
                }
                Value::Array(items)
            }
            5 => {
                let (n, w) = self.length(info)?;
                let mut entries = Vec::with_capacity(n);
                for _ in 0..n {
                    let key = self.value(depth + 1)?;
                    entries.push((key, self.value(depth + 1)?));
                }
                Value::Map(entries, w)
            }
            6 => {
                let (tag, _) = self.argument(info)?;
                Value::Tag(tag, Box::new(self.value(depth + 1)?))
            }
            _ => match info {
                20 => Value::Bool(false),
                21 => Value::Bool(true),
                22 => Value::Null,
                23 => Value::Undefined,
                25 => Value::Float(half(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))),
                26 => Value::Float(f32::from_be_bytes(self.take(4)?.try_into().unwrap()) as f64),
                27 => Value::Float(f64::from_be_bytes(self.take(8)?.try_into().unwrap())),
                other => return Err(DecodeError(format!("unsupported simple value {other}"))),
            },
        })
    }
}

/// IEEE 754 half precision, which cbor-x never writes but may read.
fn half(bits: u16) -> f64 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = ((bits >> 10) & 0x1f) as i32;
    let fraction = (bits & 0x3ff) as f64;
    sign * match exponent {
        0 => fraction * 2f64.powi(-24),
        31 if fraction == 0.0 => f64::INFINITY,
        31 => f64::NAN,
        e => (1.0 + fraction / 1024.0) * 2f64.powi(e - 15),
    }
}
