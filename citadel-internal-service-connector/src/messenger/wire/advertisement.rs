//! The capability advertisement: a few bytes after an Ack or Poll frame.
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
    frame.push(capabilities.to_wire());
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
        [m0, m1, version, caps, ..] if [*m0, *m1] == MAGIC && *version >= VERSION => {
            PeerCapabilities::from_wire(*caps)
        }
        [] => PeerCapabilities::LEGACY,
        other => {
            log::warn!(target: "ism", "[WIRE] {} unrecognised bytes after a control frame; treating the sender as legacy", other.len());
            PeerCapabilities::LEGACY
        }
    }
}
