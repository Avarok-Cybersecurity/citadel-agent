//! The capability advertisement: eight bytes after an Ack or Poll frame.
//!
//! Layout: magic (2) | version (1) | feature flags (1) | codec-id set (u32 LE).
//! The codec set has one bit per `Codec` wire id, so a peer advertises every
//! codec it can decode -- a new codec needs no layout change, only a bit.
//!
//! It goes AFTER the bincode frame because that is the one place a legacy
//! peer is guaranteed not to look. `bincode2::deserialize` stops at the end of
//! the value and never checks for trailing input (bincode 1.x semantics), so a
//! legacy build decodes the Ack or Poll exactly as before and the advertisement
//! is invisible to it -- no new variant, no failed decode, nothing forwarded to
//! its UI. The interop tests decode real advertised frames with a replica of
//! the legacy enum to hold that in place.

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

/// What the bytes after a control frame say about its sender.
///
/// Nothing at all is `LEGACY`: that is what a legacy build sends, and treating
/// it so is how a peer that reloaded onto an old build stops being sent
/// extensions. Bytes that are not an advertisement are also `LEGACY` -- a
/// frame this build cannot read an advertisement from has not advertised
/// anything this build can use.
pub(super) fn read(trailer: &[u8]) -> PeerCapabilities {
    match trailer {
        // Later versions may append; what version 1 defines is read as is.
        [m0, m1, version, flags, c0, c1, c2, c3, ..]
            if [*m0, *m1] == MAGIC && *version >= VERSION =>
        {
            PeerCapabilities::from_wire(*flags, u32::from_le_bytes([*c0, *c1, *c2, *c3]))
        }
        [] => PeerCapabilities::LEGACY,
        other => {
            log::warn!(target: "ism", "[WIRE] {} unrecognised bytes after a control frame; treating the sender as legacy", other.len());
            PeerCapabilities::LEGACY
        }
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
        assert_eq!(read(&trailer), advertised);
    }

    #[test]
    fn a_later_version_is_read_for_the_fields_it_shares() {
        let advertised = PeerCapabilities::new(false, CodecSet::of(&[Codec::Brotli]));
        let mut trailer = Vec::new();
        append(&mut trailer, advertised);
        trailer[2] = VERSION + 1;
        trailer.extend_from_slice(&[0xaa, 0xbb]);
        assert_eq!(read(&trailer), advertised);
    }

    #[test]
    fn a_truncated_or_foreign_trailer_means_legacy() {
        let mut trailer = Vec::new();
        append(
            &mut trailer,
            PeerCapabilities::new(true, CodecSet::of(&[Codec::Brotli])),
        );
        assert_eq!(read(&trailer[..7]), PeerCapabilities::LEGACY);
        assert_eq!(read(&[0u8; 8]), PeerCapabilities::LEGACY);
    }
}
