//! The compiled name tables are exactly the Unicode 17.0.0 data they were generated from: the
//! files' sha256, then every code point's simple case fold (statuses C and S) and assignment.

mod common;

use citadel_vfs::name::{is_assigned, simple_case_fold};
use sha2::{Digest, Sha256};

const CASE_FOLDING: &str = include_str!("unicode/CaseFolding-17.0.0.txt");
const DERIVED_AGE: &str = include_str!("unicode/DerivedAge-17.0.0.txt");

fn sha256(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn fields(line: &str) -> Option<Vec<&str>> {
    let data = line.split('#').next()?.trim();
    (!data.is_empty()).then(|| data.split(';').map(str::trim).collect())
}

fn cp(text: &str) -> u32 {
    u32::from_str_radix(text, 16).unwrap_or_else(|_| panic!("code point {text}"))
}

both! {
    fn data_files_are_the_published_ones() {
        assert_eq!(sha256(CASE_FOLDING), "ff8d8fefbf123574205085d6714c36149eb946d717a0c585c27f0f4ef58c4183");
        assert_eq!(sha256(DERIVED_AGE), "f8ecdf768bdc210f201abd271d9bc587825618a86a7046a8146cc816393f1998");
    }

    fn fold_table_is_case_folding_c_and_s() {
        let mut fold = std::collections::BTreeMap::new();
        for f in CASE_FOLDING.lines().filter_map(fields) {
            if f[1] == "C" || f[1] == "S" {
                fold.insert(cp(f[0]), cp(f[2]));
            }
        }
        assert!(fold.len() > 1400, "parsed {} foldings", fold.len());
        for c in (0..=0x10FFFFu32).filter_map(char::from_u32) {
            let want = fold.get(&(c as u32)).and_then(|t| char::from_u32(*t)).unwrap_or(c);
            assert_eq!(simple_case_fold(c), want, "U+{:04X}", c as u32);
        }
    }

    fn assigned_table_is_derived_age() {
        let mut assigned = vec![false; 0x110000];
        for f in DERIVED_AGE.lines().filter_map(fields) {
            let (lo, hi) = match f[0].split_once("..") {
                Some((a, b)) => (cp(a), cp(b)),
                None => (cp(f[0]), cp(f[0])),
            };
            for c in lo..=hi {
                assigned[c as usize] = true;
            }
        }
        for c in (0..=0x10FFFFu32).filter_map(char::from_u32) {
            assert_eq!(is_assigned(c), assigned[c as usize], "U+{:04X}", c as u32);
        }
    }
}
