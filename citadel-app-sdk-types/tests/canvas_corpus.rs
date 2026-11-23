//! The CW-5 draw-list corpus (`tests/contracts/batch-1116/canvas-draw-corpus.json`), which the TS
//! host decodes too: every frame's CBOR decodes to its `draw`, encodes back to the same bytes, and
//! reaches its `verdict`. The corpus must cover every command and every per-command rule.

use citadel_app_sdk_types::calls::canvas_check::{CanvasDrawError, LIMIT_CANVAS_DRAW};
use citadel_app_sdk_types::calls::CanvasDraw;
use citadel_app_sdk_types::{decode, encode};
use serde_json::{json, Value};
use std::collections::BTreeSet;

fn corpus() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/contracts/batch-1116/canvas-draw-corpus.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn unhex(s: &str) -> Vec<u8> {
    assert!(s.len().is_multiple_of(2), "odd hex");
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// The corpus's names for the rules, as the TS host reports them too.
fn verdict(result: Result<(), CanvasDrawError>) -> Value {
    let (rule, command) = match result {
        Ok(()) => return json!({"ok": null}),
        Err(CanvasDrawError::PathPoints { command, .. }) => ("path_points", command),
        Err(CanvasDrawError::PathPaintsNothing { command }) => ("path_paints_nothing", command),
        Err(CanvasDrawError::RegionId { command }) => ("region_id", command),
        Err(CanvasDrawError::RegionLabel { command }) => ("region_label", command),
        Err(e @ CanvasDrawError::TooManyRegions { .. }) => panic!("not stored: {e:?}"),
    };
    json!({"refused": LIMIT_CANVAS_DRAW, "rule": rule, "command": command})
}

#[test]
fn every_frame_decodes_from_its_cbor_re_encodes_to_it_and_reaches_its_verdict() {
    let corpus = corpus();
    let frames = corpus["frames"].as_array().unwrap();
    assert!(frames.len() >= 10, "a corpus of {} frames", frames.len());
    for f in frames {
        let name = f["name"].as_str().unwrap();
        let draw: CanvasDraw = serde_json::from_value(f["draw"].clone()).unwrap();
        let bytes = unhex(f["cbor_hex"].as_str().unwrap());
        assert_eq!(decode::<CanvasDraw>(&bytes).unwrap(), draw, "{name}");
        assert_eq!(encode(&draw), bytes, "{name}");
        assert_eq!(verdict(draw.validate()), f["verdict"], "{name}");
    }
}

#[test]
fn the_corpus_covers_every_command_and_every_per_command_rule() {
    let corpus = corpus();
    let frames = corpus["frames"].as_array().unwrap();
    let mut tags = BTreeSet::new();
    let mut rules = BTreeSet::new();
    for f in frames {
        for c in f["draw"]["commands"].as_array().unwrap() {
            tags.extend(c.as_object().unwrap().keys().cloned());
        }
        if let Some(rule) = f["verdict"]["rule"].as_str() {
            rules.insert(rule.to_string());
        }
    }
    let every = [
        "arc",
        "clear",
        "fill_circle",
        "fill_rect",
        "fill_text",
        "line",
        "path",
        "region",
        "stroke_circle",
        "stroke_rect",
    ];
    assert_eq!(tags, every.map(String::from).into());
    let per_command = [
        "path_paints_nothing",
        "path_points",
        "region_id",
        "region_label",
    ];
    assert_eq!(rules, per_command.map(String::from).into());
}
