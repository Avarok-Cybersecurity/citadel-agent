//! CBOR exactly as the web UI's cbor-x (1.6.0, default options) writes it.
//!
//! The UI encodes every P2P command and every stored reaction list with cbor-x,
//! and peers' UIs decode what the agent sends with it, so the agent must produce
//! the same bytes and read anything cbor-x produces. The rules here are not
//! reasoned from the library: they are pinned by fixtures the real encoder
//! wrote (tests/fixtures/p2p_commands, `generate.mjs`), each decoded and
//! re-encoded to identical bytes.
//!
//! What cbor-x does, and so what [`Value::object`] and [`Value::number`] do:
//! * a JS object is a map with a 16-bit length header (`0xb9 NN NN`), keys in
//!   insertion order, and a key whose value is `undefined` kept, as `0xf7`;
//! * a `bigint` is always the 8-byte form (`0x1b`/`0x3b`), however small;
//! * a `number` that is an integer in the uint32 or int32 range is the minimal
//!   integer; any other number -- `Date.now()` included -- is a float64;
//! * strings, byte strings and arrays use minimal length headers.
//!
//! The decoder keeps each header's width, so whatever it reads re-encodes to the
//! bytes it came from.

/// How many bytes followed an initial byte's 5-bit argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Width {
    /// The value or length is in the initial byte itself (< 24).
    Inline,
    One,
    Two,
    Four,
    Eight,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    /// Major type 0.
    Uint(u64, Width),
    /// Major type 1: the value is `-1 - n`.
    Nint(u64, Width),
    Bytes(Vec<u8>),
    Text(String),
    Array(Vec<Value>),
    /// Keys in order; the header width is kept (cbor-x always writes `Two`).
    Map(Vec<(Value, Value)>, Width),
    Tag(u64, Box<Value>),
    Bool(bool),
    Null,
    Undefined,
    /// Always written as float64; read from any float width.
    Float(f64),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct DecodeError(pub String);

impl Value {
    /// A JS `bigint`.
    pub(crate) fn bigint(value: u64) -> Self {
        Value::Uint(value, Width::Eight)
    }

    /// A JS `number`, encoded as cbor-x encodes one.
    pub(crate) fn number(value: f64) -> Self {
        let is_integer = value.fract() == 0.0 && value.is_finite();
        if is_integer && (0.0..=u32::MAX as f64).contains(&value) {
            let n = value as u64;
            Value::Uint(n, minimal(n))
        } else if is_integer && (i32::MIN as f64..0.0).contains(&value) {
            let n = (-1.0 - value) as u64;
            Value::Nint(n, minimal(n))
        } else {
            Value::Float(value)
        }
    }

    pub(crate) fn text(value: impl Into<String>) -> Self {
        Value::Text(value.into())
    }

    /// A JS object literal: `(key, value)` in insertion order.
    pub(crate) fn object(fields: Vec<(&str, Value)>) -> Self {
        Value::Map(
            fields
                .into_iter()
                .map(|(k, v)| (Value::text(k), v))
                .collect(),
            Width::Two,
        )
    }

    /// The value under `key`, if this is a map with a text key `key`.
    pub(crate) fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Map(entries, _) => entries
                .iter()
                .find(|(k, _)| matches!(k, Value::Text(t) if t == key))
                .map(|(_, v)| v),
            _ => None,
        }
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Value::Text(t) => Some(t),
            _ => None,
        }
    }

    /// A JS number, as JS reads it (integers and floats alike).
    pub(crate) fn as_number(&self) -> Option<f64> {
        match self {
            Value::Uint(n, w) if *w != Width::Eight => Some(*n as f64),
            Value::Nint(n, w) if *w != Width::Eight => Some(-1.0 - *n as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// A JS bigint: only the 8-byte form decodes as one in cbor-x.
    pub(crate) fn as_bigint(&self) -> Option<u64> {
        match self {
            Value::Uint(n, Width::Eight) => Some(*n),
            _ => None,
        }
    }

    pub(crate) fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut Vec<u8>) {
        match self {
            Value::Uint(n, w) => head(out, 0, *n, *w),
            Value::Nint(n, w) => head(out, 1, *n, *w),
            Value::Bytes(b) => {
                head(out, 2, b.len() as u64, minimal(b.len() as u64));
                out.extend_from_slice(b);
            }
            Value::Text(t) => {
                head(out, 3, t.len() as u64, minimal(t.len() as u64));
                out.extend_from_slice(t.as_bytes());
            }
            Value::Array(items) => {
                head(out, 4, items.len() as u64, minimal(items.len() as u64));
                items.iter().for_each(|item| item.write(out));
            }
            Value::Map(entries, w) => {
                head(out, 5, entries.len() as u64, *w);
                for (k, v) in entries {
                    k.write(out);
                    v.write(out);
                }
            }
            Value::Tag(tag, inner) => {
                head(out, 6, *tag, minimal(*tag));
                inner.write(out);
            }
            Value::Bool(false) => out.push(0xf4),
            Value::Bool(true) => out.push(0xf5),
            Value::Null => out.push(0xf6),
            Value::Undefined => out.push(0xf7),
            Value::Float(f) => {
                out.push(0xfb);
                out.extend_from_slice(&f.to_be_bytes());
            }
        }
    }
}

fn minimal(n: u64) -> Width {
    match n {
        0..=23 => Width::Inline,
        24..=0xff => Width::One,
        0x100..=0xffff => Width::Two,
        0x1_0000..=0xffff_ffff => Width::Four,
        _ => Width::Eight,
    }
}

pub(super) fn head(out: &mut Vec<u8>, major: u8, n: u64, width: Width) {
    let m = major << 5;
    match width {
        Width::Inline => out.push(m | n as u8),
        Width::One => out.extend_from_slice(&[m | 24, n as u8]),
        Width::Two => {
            out.push(m | 25);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        Width::Four => {
            out.push(m | 26);
            out.extend_from_slice(&(n as u32).to_be_bytes());
        }
        Width::Eight => {
            out.push(m | 27);
            out.extend_from_slice(&n.to_be_bytes());
        }
    }
}

#[path = "cbor_decode.rs"]
mod decode;

#[cfg(test)]
#[path = "cbor_tests.rs"]
mod tests;
