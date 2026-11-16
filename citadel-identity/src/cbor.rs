//! Deterministic CBOR (RFC 8949 §4.2.1, core deterministic encoding) for the few shapes the
//! well-known document's signature covers: unsigned integers, text, arrays and text-keyed maps.
//! Integers and lengths take their shortest head; map entries are sorted by the bytewise order
//! of their encoded keys; a duplicate key is refused.

pub(crate) enum Cbor<'a> {
    Unsigned(u64),
    Text(&'a str),
    Array(Vec<Cbor<'a>>),
    Map(Vec<(&'a str, Cbor<'a>)>),
}

const UNSIGNED: u8 = 0;
const TEXT: u8 = 3;
const ARRAY: u8 = 4;
const MAP: u8 = 5;

impl Cbor<'_> {
    pub(crate) fn encode(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        self.encode_into(&mut out)?;
        Ok(out)
    }

    fn encode_into(&self, out: &mut Vec<u8>) -> Result<(), String> {
        match self {
            Self::Unsigned(n) => head(out, UNSIGNED, *n),
            Self::Text(s) => {
                head(out, TEXT, s.len() as u64);
                out.extend_from_slice(s.as_bytes());
            }
            Self::Array(items) => {
                head(out, ARRAY, items.len() as u64);
                for item in items {
                    item.encode_into(out)?;
                }
            }
            Self::Map(entries) => {
                let mut encoded = Vec::with_capacity(entries.len());
                for (key, value) in entries {
                    let key_bytes = Self::Text(key).encode()?;
                    encoded.push((key_bytes, value.encode()?));
                }
                encoded.sort_by(|a, b| a.0.cmp(&b.0));
                if encoded.windows(2).any(|pair| pair[0].0 == pair[1].0) {
                    return Err("a map key repeats".to_string());
                }
                head(out, MAP, encoded.len() as u64);
                for (key, value) in encoded {
                    out.extend_from_slice(&key);
                    out.extend_from_slice(&value);
                }
            }
        }
        Ok(())
    }
}

fn head(out: &mut Vec<u8>, major: u8, n: u64) {
    let major = major << 5;
    if n < 24 {
        out.push(major | n as u8);
    } else if let Ok(n) = u8::try_from(n) {
        out.extend_from_slice(&[major | 24, n]);
    } else if let Ok(n) = u16::try_from(n) {
        out.push(major | 25);
        out.extend_from_slice(&n.to_be_bytes());
    } else if let Ok(n) = u32::try_from(n) {
        out.push(major | 26);
        out.extend_from_slice(&n.to_be_bytes());
    } else {
        out.push(major | 27);
        out.extend_from_slice(&n.to_be_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::Cbor;

    #[test]
    fn heads_are_shortest_and_map_keys_sort_by_encoded_bytes() {
        assert_eq!(Cbor::Unsigned(23).encode().unwrap(), [0x17]);
        assert_eq!(Cbor::Unsigned(24).encode().unwrap(), [0x18, 24]);
        assert_eq!(Cbor::Unsigned(256).encode().unwrap(), [0x19, 1, 0]);
        assert_eq!(
            Cbor::Unsigned(1_794_700_000).encode().unwrap(),
            [0x1a, 0x6a, 0xf8, 0xf2, 0xe0]
        );
        assert_eq!(Cbor::Text("a").encode().unwrap(), [0x61, b'a']);
        // RFC 8949 §4.2.1: "b" (0x61 0x62) sorts before "aa" (0x62 0x61 0x61).
        let map = Cbor::Map(vec![("aa", Cbor::Unsigned(1)), ("b", Cbor::Unsigned(2))]);
        assert_eq!(
            map.encode().unwrap(),
            [0xa2, 0x61, b'b', 0x02, 0x62, b'a', b'a', 0x01]
        );
        let dup = Cbor::Map(vec![("a", Cbor::Unsigned(1)), ("a", Cbor::Unsigned(2))]);
        assert!(dup.encode().is_err());
        assert_eq!(
            Cbor::Array(vec![Cbor::Text("x")]).encode().unwrap(),
            [0x81, 0x61, b'x']
        );
    }
}
