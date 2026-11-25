//! `fixtures/names`: the name grammar, NFC, and the simple case fold.

mod common;

use citadel_vfs::name::{check, fold_key, Name, NameError};
use common::parse;

fn why(e: NameError) -> &'static str {
    match e {
        NameError::Empty => "empty",
        NameError::Dot => "dot",
        NameError::Slash => "slash",
        NameError::Control => "Cc",
        NameError::NotNfc => "nfc",
        NameError::TooLong => "long",
        NameError::Unassigned => "unassigned",
    }
}

fn expected(text: &str) -> &'static str {
    match text {
        "empty" => "empty",
        "dot" | "dot-dot" => "dot",
        "slash" => "slash",
        "Cc" => "Cc",
        t if t.starts_with("not NFC") => "nfc",
        t if t.contains("bytes") || t.contains("NAME_MAX") => "long",
        t if t.contains("unassigned") => "unassigned",
        other => panic!("unknown why {other}"),
    }
}

both! {
    fn every_name_case() {
        let f = parse(include_str!("contracts/rel-1124/names/cases.json"));
        for v in f["valid"].as_array().into_iter().flatten() {
            let s = v.as_str().unwrap_or_default();
            assert_eq!(check(s), Ok(()), "valid: {s:?}");
        }
        for i in f["invalid"].as_array().into_iter().flatten() {
            let s = i["name"].as_str().unwrap_or_default();
            let want = expected(i["why"].as_str().unwrap_or_default());
            assert_eq!(check(s).map_err(why), Err(want), "invalid: {s:?} ({})", i["why"]);
        }
        for pair in f["collide"].as_array().into_iter().flatten() {
            let (a, b) = (pair[0].as_str().unwrap_or_default(), pair[1].as_str().unwrap_or_default());
            assert_eq!(fold_key(a), fold_key(b), "collide: {a:?} {b:?}");
        }
        for pair in f["distinct"].as_array().into_iter().flatten() {
            let (a, b) = (pair[0].as_str().unwrap_or_default(), pair[1].as_str().unwrap_or_default());
            assert_ne!(fold_key(a), fold_key(b), "distinct: {a:?} {b:?} ({})", pair[2]);
        }
    }

    fn a_name_keeps_its_case() {
        let name = Name::new("Report.md").unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(name.as_str(), "Report.md");
        assert_eq!(name.fold_key(), "report.md");
    }
}
