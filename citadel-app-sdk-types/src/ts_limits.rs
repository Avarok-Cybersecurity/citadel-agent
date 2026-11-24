//! The limits and versions of `bindings/AbiConstants.ts`: what the host enforces, by the names
//! the Rust uses. Part of [`crate::ts_constants::render`].

use crate::abi;
use crate::calls::{canvas_check as canvas, session, topic};
use std::fmt::Write;
use std::string::String;

fn q(s: &str) -> String {
    format!("{s:?}")
}

pub(crate) fn render(o: &mut String) {
    for (name, value) in [
        ("MAX_TOPICS", topic::MAX_TOPICS as u32),
        ("MAX_TOPIC_NAME_LEN", topic::MAX_TOPIC_NAME_LEN as u32),
        (
            "TOPIC_MAX_MSG_BYTES_CEILING",
            topic::TOPIC_MAX_MSG_BYTES_CEILING,
        ),
        (
            "TOPIC_MAX_MSGS_PER_S_CEILING",
            topic::TOPIC_MAX_MSGS_PER_S_CEILING,
        ),
        ("TOPIC_ENVELOPE_OVERHEAD", topic::TOPIC_ENVELOPE_OVERHEAD),
    ] {
        writeln!(o, "export const {name} = {value} as const;").unwrap();
    }
    for (name, value) in [
        ("LIMIT_TOPIC_RATE", topic::LIMIT_TOPIC_RATE),
        ("LIMIT_TOPIC_BYTES", topic::LIMIT_TOPIC_BYTES),
        ("TOPIC_ENVELOPE_TAG", topic::TOPIC_ENVELOPE_TAG),
    ] {
        writeln!(o, "export const {name} = {} as const;", q(value)).unwrap();
    }
    for (name, value) in [
        ("ABI_MINOR", abi::ABI_MINOR),
        ("ABI_MINOR_CANVAS_EXT", abi::ABI_MINOR_CANVAS_EXT),
        ("MAX_SESSION_SERVERS", session::MAX_SESSION_SERVERS as u32),
        (
            "SESSION_MAX_MSG_BYTES_CEILING",
            session::SESSION_MAX_MSG_BYTES_CEILING,
        ),
        (
            "SESSION_MAX_MSGS_PER_S_CEILING",
            session::SESSION_MAX_MSGS_PER_S_CEILING,
        ),
        ("PATH_MIN_POINTS", canvas::PATH_MIN_POINTS as u32),
        ("PATH_MAX_POINTS", canvas::PATH_MAX_POINTS as u32),
        ("REGION_ID_MAX_LEN", canvas::REGION_ID_MAX_LEN as u32),
        (
            "REGION_LABEL_MAX_CHARS",
            canvas::REGION_LABEL_MAX_CHARS as u32,
        ),
        (
            "MAX_REGIONS_PER_FRAME",
            canvas::MAX_REGIONS_PER_FRAME as u32,
        ),
    ] {
        writeln!(o, "export const {name} = {value} as const;").unwrap();
    }
    for (name, value) in [
        ("LIMIT_SESSION_BYTES", session::LIMIT_SESSION_BYTES),
        ("LIMIT_SESSION_RATE", session::LIMIT_SESSION_RATE),
        ("LIMIT_CANVAS_DRAW", canvas::LIMIT_CANVAS_DRAW),
    ] {
        writeln!(o, "export const {name} = {} as const;", q(value)).unwrap();
    }
}
