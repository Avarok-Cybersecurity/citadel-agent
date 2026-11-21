//! The capability advertisement: eight bytes after an Ack or Poll frame.
//!
//! Layout: magic (2) | version (1) | feature flags (1) | codec-id set (u32 LE).
//! The codec set has one bit per `Codec` wire id, so a peer advertises every
//! codec it can decode -- a new codec needs no layout change, only a bit.
//!
//! Every Ack and Poll carries one. It goes after the bincode frame:
//! `bincode2::deserialize_from` stops at the end of the value, so what is left
//! is exactly the trailer.

use intersession_layer_messaging::PeerCapabilities;

/// Marks the trailer as ours rather than stray bytes.
const MAGIC: [u8; 2] = [0xC1, 0x7A];
/// Bumped only if the layout after it changes; later versions may append.
const VERSION: u8 = 1;

pub(super) fn append(frame: &mut Vec<u8>, capabilities: PeerCapabilities) {
    frame.extend_from_slice(&MAGIC);
    frame.push(VERSION);
    frame.push(capabilities.flags_to_wire());
    frame.extend_from_slice(&capabilities.codecs().to_wire().to_le_bytes());
}

/// What the bytes after a control frame say about its sender, or why they
/// say nothing: a missing or unrecognised trailer is refused, never read as a
/// peer that offers less.
pub(super) fn read(trailer: &[u8]) -> Result<PeerCapabilities, String> {
    match trailer {
        // Later versions may append; what version 1 defines is read as is.
        [m0, m1, version, flags, c0, c1, c2, c3, ..]
            if [*m0, *m1] == MAGIC && *version >= VERSION =>
        {
            Ok(PeerCapabilities::from_wire(
                *flags,
                u32::from_le_bytes([*c0, *c1, *c2, *c3]),
            ))
        }
        [] => Err("no capability advertisement".to_string()),
        other => Err(format!(
            "{} bytes after it are not a capability advertisement",
            other.len()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use intersession_layer_messaging::compression::{Codec, CodecSet};

    #[test]
    fn the_codec_set_round_trips_including_reserved_ids() {
        let advertised = PeerCapabilities::new(true, CodecSet::of(&[Codec::Rill, Codec::Deflate]));
        let mut trailer = Vec::new();
        append(&mut trailer, advertised);
        assert_eq!(trailer.len(), 8);
        assert_eq!(read(&trailer), Ok(advertised));
    }

    #[test]
    fn a_later_version_is_read_for_the_fields_it_shares() {
        let advertised = PeerCapabilities::new(false, CodecSet::of(&[Codec::Brotli]));
        let mut trailer = Vec::new();
        append(&mut trailer, advertised);
        trailer[2] = VERSION + 1;
        trailer.extend_from_slice(&[0xaa, 0xbb]);
        assert_eq!(read(&trailer), Ok(advertised));
    }

    #[test]
    fn a_missing_truncated_or_foreign_trailer_is_refused() {
        let mut trailer = Vec::new();
        append(
            &mut trailer,
            PeerCapabilities::new(true, CodecSet::of(&[Codec::Brotli])),
        );
        assert!(read(&[]).is_err());
        assert!(read(&trailer[..7]).is_err());
        assert!(read(&[0u8; 8]).is_err());
    }
}
