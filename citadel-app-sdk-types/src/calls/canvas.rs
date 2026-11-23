//! `canvas`: the host-owned canvas the app paints into. Coordinates are CSS pixels; colours are
//! `0xRRGGBBAA`.

use alloc::string::String;
use alloc::vec::Vec;

abi_types! {
    /// The canvas's current size.
    pub struct CanvasMeasure {}

    pub struct CanvasSize {
        pub width: u32,
        pub height: u32,
        pub dpr: f32,
    }

    /// Paint these commands, in order, as one frame.
    pub struct CanvasDraw {
        pub commands: Vec<DrawCommand>,
    }

    #[serde(rename_all = "snake_case")]
    pub enum DrawCommand {
        Clear { color: u32 },
        FillRect { x: f32, y: f32, w: f32, h: f32, color: u32 },
        StrokeRect { x: f32, y: f32, w: f32, h: f32, line_width: f32, color: u32 },
        Line { x1: f32, y1: f32, x2: f32, y2: f32, line_width: f32, color: u32 },
        /// Text is drawn as text, in the host's font; never parsed as markup.
        FillText { x: f32, y: f32, text: String, px: f32, color: u32 },
        // ABI minor 1 (CW-5): `abi::ABI_MINOR_CANVAS_EXT`. Limits: `canvas_check`.
        FillCircle { cx: f32, cy: f32, r: f32, color: u32 },
        StrokeCircle { cx: f32, cy: f32, r: f32, line_width: f32, color: u32 },
        /// 2 to 256 points; `stroke` is `(line_width, color)`. At least one of `fill`, `stroke`.
        Path {
            points: Vec<(f32, f32)>,
            closed: bool,
            fill: Option<u32>,
            stroke: Option<(f32, u32)>,
        },
        /// Radians, clockwise from +x.
        Arc { cx: f32, cy: f32, r: f32, start: f32, end: f32, line_width: f32, color: u32 },
        /// Paints nothing: one focusable element over the rectangle, named `label`, whose
        /// activation is `AppEvent::Activate { id }`. Each frame's regions replace the last's.
        Region { id: String, x: f32, y: f32, w: f32, h: f32, label: String, role: RegionRole },
    }

    /// The accessible role of a `Region`.
    #[serde(rename_all = "snake_case")]
    pub enum RegionRole {
        Button,
        Link,
        Img,
        Group,
    }

    pub struct MeasureText {
        pub text: String,
        pub px: f32,
    }

    pub struct TextMetrics {
        pub width: f32,
    }
}
