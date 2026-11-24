//! The limits a host enforces on one `CanvasDraw` before painting any of it (CW-5). A frame that
//! breaks one is refused whole (`Refused::Limit(LIMIT_CANVAS_DRAW)`): nothing of it is painted.

use super::{CanvasDraw, DrawCommand};
use crate::Refused;

/// A `Path` has at least this many points.
pub const PATH_MIN_POINTS: usize = 2;
/// A `Path` has at most this many points.
pub const PATH_MAX_POINTS: usize = 256;
/// A `Region` id is 1 to this many of `[a-z0-9_-]`.
pub const REGION_ID_MAX_LEN: usize = 64;
/// A `Region` label is at most this many characters (Unicode scalar values, not UTF-16 units).
pub const REGION_LABEL_MAX_CHARS: usize = 256;
/// One frame holds at most this many `Region`s.
pub const MAX_REGIONS_PER_FRAME: usize = 512;
/// `Refused::Limit` of a `CanvasDraw` that breaks any rule here.
pub const LIMIT_CANVAS_DRAW: &str = "canvas_draw";

/// Why a `CanvasDraw` is refused; `command` is the index of the offending command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CanvasDrawError {
    PathPoints { command: usize, points: usize },
    PathPaintsNothing { command: usize },
    RegionId { command: usize },
    RegionLabel { command: usize },
    TooManyRegions { regions: usize },
}

impl CanvasDrawError {
    /// What the host answers the call with.
    pub fn refusal(&self) -> Refused {
        Refused::Limit(LIMIT_CANVAS_DRAW.into())
    }
}

/// Whether `id` is a region id: 1-64 of `[a-z0-9_-]`.
pub fn is_region_id(id: &str) -> bool {
    (1..=REGION_ID_MAX_LEN).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

fn check_command(command: usize, c: &DrawCommand) -> Result<bool, CanvasDrawError> {
    match c {
        DrawCommand::Path {
            points,
            fill,
            stroke,
            ..
        } => {
            if !(PATH_MIN_POINTS..=PATH_MAX_POINTS).contains(&points.len()) {
                let points = points.len();
                return Err(CanvasDrawError::PathPoints { command, points });
            }
            if fill.is_none() && stroke.is_none() {
                return Err(CanvasDrawError::PathPaintsNothing { command });
            }
            Ok(false)
        }
        DrawCommand::Region { id, label, .. } => {
            if !is_region_id(id) {
                return Err(CanvasDrawError::RegionId { command });
            }
            if label.chars().count() > REGION_LABEL_MAX_CHARS {
                return Err(CanvasDrawError::RegionLabel { command });
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

impl CanvasDraw {
    /// The first rule the frame breaks, in command order; the region count is judged last.
    pub fn validate(&self) -> Result<(), CanvasDrawError> {
        let mut regions = 0usize;
        for (i, c) in self.commands.iter().enumerate() {
            if check_command(i, c)? {
                regions += 1;
            }
        }
        if regions > MAX_REGIONS_PER_FRAME {
            return Err(CanvasDrawError::TooManyRegions { regions });
        }
        Ok(())
    }
}
