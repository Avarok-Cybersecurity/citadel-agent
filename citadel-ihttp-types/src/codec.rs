//! CBOR in and out. Both directions validate, so no side can write a frame the other refuses.

use crate::validate::{validate, within_frame_size, FrameError};
use crate::IhttpFrame;

/// The frame as CBOR, refused if it breaks a limit.
pub fn encode(frame: &IhttpFrame) -> Result<Vec<u8>, FrameError> {
    validate(frame)?;
    let mut out = Vec::new();
    ciborium::ser::into_writer(frame, &mut out)
        .map_err(|why| FrameError::Malformed(why.to_string()))?;
    within_frame_size(&out)?;
    Ok(out)
}

/// One frame from exactly `bytes`: at most `MAX_FRAME_BYTES`, one CBOR value with nothing
/// after it, of a known kind with exactly its fields, within every limit.
pub fn decode(bytes: &[u8]) -> Result<IhttpFrame, FrameError> {
    within_frame_size(bytes)?;
    let mut rest = bytes;
    let frame: IhttpFrame = ciborium::de::from_reader(&mut rest)
        .map_err(|why| FrameError::Malformed(why.to_string()))?;
    if !rest.is_empty() {
        return Err(FrameError::Malformed(format!(
            "{} bytes after the frame",
            rest.len()
        )));
    }
    validate(&frame)?;
    Ok(frame)
}
