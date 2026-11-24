//! `canvas`: paint the host-owned canvas. Colours are `0xRRGGBBAA`.
//!
//! The circle, path, arc and `Region` commands and `AppEvent::Activate` are ABI minor 1: paint
//! them only when `AppContext::has_canvas_extension` says the host has them; an older host
//! refuses the frame.
//! A frame that breaks a limit of [`canvas_check`] is refused whole.

use crate::call_now as call;
use citadel_app_sdk_types::calls::{CanvasDraw, CanvasMeasure, CanvasSize, MeasureText};
use citadel_app_sdk_types::Refused;

pub use citadel_app_sdk_types::calls::canvas_check;
pub use citadel_app_sdk_types::calls::{DrawCommand, RegionRole};

pub fn size() -> Result<CanvasSize, Refused> {
    call(&CanvasMeasure {})
}

/// Paint `commands`, in order, as one frame.
pub fn draw(commands: Vec<DrawCommand>) -> Result<(), Refused> {
    call(&CanvasDraw { commands })
}

pub fn fill_rect(x: f32, y: f32, w: f32, h: f32, color: u32) -> Result<(), Refused> {
    draw(vec![DrawCommand::FillRect { x, y, w, h, color }])
}

pub fn text_width(text: &str, px: f32) -> Result<f32, Refused> {
    call(&MeasureText {
        text: text.into(),
        px,
    })
    .map(|m| m.width)
}
