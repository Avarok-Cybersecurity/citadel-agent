//! The CW-5 canvas commands (canvas-ext-abi.md §1-§2): their wire forms, and the limits a host
//! refuses a whole frame for. Every refusal is paired with the nearest frame that is accepted.

use citadel_app_sdk_types::calls::canvas_check::*;
use citadel_app_sdk_types::calls::{CanvasDraw, DrawCommand, RegionRole};
use citadel_app_sdk_types::{abi, decode, encode, AppEvent, Refused};
use serde_json::json;

fn json<T: serde::Serialize>(v: &T) -> serde_json::Value {
    serde_json::to_value(v).unwrap()
}

fn path(n: usize, fill: Option<u32>, stroke: Option<(f32, u32)>) -> DrawCommand {
    let points = (0..n).map(|i| (i as f32, 0.5)).collect();
    DrawCommand::Path {
        points,
        closed: false,
        fill,
        stroke,
    }
}

fn region(id: &str, label: &str) -> DrawCommand {
    DrawCommand::Region {
        id: id.into(),
        x: 0.0,
        y: 0.0,
        w: 8.0,
        h: 8.0,
        label: label.into(),
        role: RegionRole::Button,
    }
}

fn frame(commands: Vec<DrawCommand>) -> Result<(), CanvasDrawError> {
    CanvasDraw { commands }.validate()
}

#[test]
fn the_new_commands_have_the_contracts_wire_forms() {
    let circle = DrawCommand::FillCircle {
        cx: 1.0,
        cy: 2.0,
        r: 3.0,
        color: 0xff0000ff,
    };
    let fill_circle =
        json!({"fill_circle": {"cx": 1.0, "cy": 2.0, "r": 3.0, "color": 0xff0000ffu32}});
    assert_eq!(json(&circle), fill_circle);
    let p = DrawCommand::Path {
        points: vec![(0.0, 1.0), (2.0, 3.0)],
        closed: true,
        fill: None,
        stroke: Some((1.5, 7)),
    };
    let path_wire = json!({"path": {"points": [[0.0, 1.0], [2.0, 3.0]], "closed": true, "fill": null, "stroke": [1.5, 7]}});
    assert_eq!(json(&p), path_wire);
    let r = region("node-1", "Alice");
    let region_wire = json!({"region": {"id": "node-1", "x": 0.0, "y": 0.0, "w": 8.0, "h": 8.0, "label": "Alice", "role": "button"}});
    assert_eq!(json(&r), region_wire);
    let arc = DrawCommand::Arc {
        cx: 0.0,
        cy: 0.0,
        r: 1.0,
        start: 0.0,
        end: 3.5,
        line_width: 1.0,
        color: 1,
    };
    assert!(json(&arc)["arc"]["end"] == 3.5);
    let stroke_circle = DrawCommand::StrokeCircle {
        cx: 0.0,
        cy: 0.0,
        r: 1.0,
        line_width: 2.0,
        color: 1,
    };
    assert!(json(&stroke_circle)["stroke_circle"]["line_width"] == 2.0);
    for c in [circle, p, r, arc, stroke_circle] {
        assert_eq!(decode::<DrawCommand>(&encode(&c)).unwrap(), c);
    }
    let roles = [
        RegionRole::Button,
        RegionRole::Link,
        RegionRole::Img,
        RegionRole::Group,
    ];
    let names: Vec<serde_json::Value> = roles.iter().map(json).collect();
    assert_eq!(
        names,
        [json!("button"), json!("link"), json!("img"), json!("group")]
    );
}

#[test]
fn activate_is_tagged_activate_with_its_region_id() {
    let e = AppEvent::Activate { id: "n1".into() };
    assert_eq!(encode(&e), b"\xa1\x68activate\xa1\x62id\x62n1".as_slice());
    assert_eq!(decode::<AppEvent>(&encode(&e)).unwrap(), e);
}

#[test]
fn the_canvas_extension_is_abi_minor_1_of_abi_1() {
    assert_eq!(abi::ABI_MINOR_CANVAS_EXT, 1);
    assert_eq!(abi::ABI_MINOR, 1, "the minor this crate defines");
    assert_eq!(abi::ABI_ID, "citadel-app-abi-1");
}

#[test]
fn a_path_needs_2_to_256_points() {
    let stroke = Some((1.0, 1));
    assert_eq!(
        frame(vec![path(257, None, stroke)]),
        Err(CanvasDrawError::PathPoints {
            command: 0,
            points: 257
        })
    );
    assert_eq!(frame(vec![path(256, None, stroke)]), Ok(()));
    let one = frame(vec![region("a", "A"), path(1, None, stroke)]);
    assert_eq!(
        one,
        Err(CanvasDrawError::PathPoints {
            command: 1,
            points: 1
        })
    );
    assert_eq!(frame(vec![path(2, None, stroke)]), Ok(()));
}

#[test]
fn a_path_without_fill_or_stroke_is_refused() {
    let none = frame(vec![path(3, None, None)]);
    assert_eq!(none, Err(CanvasDrawError::PathPaintsNothing { command: 0 }));
    assert_eq!(frame(vec![path(3, Some(1), None)]), Ok(()));
    assert_eq!(frame(vec![path(3, None, Some((1.0, 1)))]), Ok(()));
}

#[test]
fn a_region_id_is_1_to_64_of_lowercase_digits_dash_underscore() {
    for bad in ["", "Node", "a b", "é", &"a".repeat(65)] {
        let got = frame(vec![region(bad, "x")]);
        assert_eq!(
            got,
            Err(CanvasDrawError::RegionId { command: 0 }),
            "{bad:?}"
        );
    }
    for good in ["a", "node_1-x", &"z".repeat(64)] {
        assert_eq!(frame(vec![region(good, "x")]), Ok(()), "{good:?}");
    }
}

#[test]
fn a_region_label_is_at_most_256_characters_counted_as_characters() {
    let long = frame(vec![region("a", &"x".repeat(257))]);
    assert_eq!(long, Err(CanvasDrawError::RegionLabel { command: 0 }));
    assert_eq!(frame(vec![region("a", &"x".repeat(256))]), Ok(()));
    assert_eq!(
        frame(vec![region("a", &"\u{1F600}".repeat(256))]),
        Ok(()),
        "256 characters of 4 bytes (and 2 UTF-16 units) each"
    );
}

#[test]
fn a_frame_holds_at_most_512_regions() {
    let many = |n: usize| (0..n).map(|_| region("r", "R")).collect::<Vec<_>>();
    assert_eq!(
        frame(many(513)),
        Err(CanvasDrawError::TooManyRegions { regions: 513 })
    );
    assert_eq!(frame(many(512)), Ok(()));
}

#[test]
fn a_valid_mixed_frame_is_accepted_and_every_refusal_is_the_draw_limit() {
    let mixed = vec![
        DrawCommand::Clear { color: 0 },
        DrawCommand::FillRect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
            color: 1,
        },
        DrawCommand::FillCircle {
            cx: 1.0,
            cy: 1.0,
            r: 1.0,
            color: 2,
        },
        path(4, Some(3), Some((1.0, 4))),
        region("n1", "Node 1"),
    ];
    assert_eq!(frame(mixed), Ok(()));
    let err = CanvasDrawError::PathPaintsNothing { command: 0 };
    assert_eq!(err.refusal(), Refused::Limit("canvas_draw".into()));
    assert_eq!(LIMIT_CANVAS_DRAW, "canvas_draw");
    assert_eq!(
        (PATH_MIN_POINTS, PATH_MAX_POINTS, REGION_ID_MAX_LEN),
        (2, 256, 64)
    );
    assert_eq!((REGION_LABEL_MAX_CHARS, MAX_REGIONS_PER_FRAME), (256, 512));
}
