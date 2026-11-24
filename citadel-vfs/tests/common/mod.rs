//! Shared by the fixture tests: the fixtures' JSON model to the crate's types and back.
#![allow(dead_code)]

pub mod build;
pub mod fold;
pub mod show;
pub mod trees;

pub use serde_json::Value as J;

/// A test on both targets: `#[test]` natively, `wasm_bindgen_test` on wasm32.
#[macro_export]
macro_rules! both {
    ($(fn $name:ident() $body:block)*) => {$(
        #[cfg_attr(not(target_arch = "wasm32"), test)]
        #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
        fn $name() $body
    )*};
}

pub fn parse(text: &str) -> J {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("fixture is not JSON: {e}"))
}

pub fn hex<const N: usize>(text: &str) -> [u8; N] {
    citadel_vfs::from_hex::<N>(text).unwrap_or_else(|| panic!("not {N} bytes of hex: {text}"))
}

pub fn hex_vec(text: &str) -> Vec<u8> {
    assert!(text.len() % 2 == 0, "odd hex {text}");
    (0..text.len() / 2)
        .map(|i| {
            u8::from_str_radix(&text[2 * i..2 * i + 2], 16).unwrap_or_else(|_| panic!("hex {text}"))
        })
        .collect()
}

/// Every order of `n` items when there are at most 6; otherwise every rotation and its reverse.
pub fn orders(n: usize) -> Vec<Vec<usize>> {
    if n <= 6 {
        let mut all: Vec<Vec<usize>> = vec![Vec::new()];
        for _ in 0..n {
            let mut next = Vec::new();
            for p in &all {
                for i in (0..n).filter(|i| !p.contains(i)) {
                    next.push([p.clone(), vec![i]].concat());
                }
            }
            all = next;
        }
        return all;
    }
    (0..n)
        .flat_map(|r| {
            let rotated: Vec<usize> = (0..n).map(|i| (i + r) % n).collect();
            let reversed: Vec<usize> = rotated.iter().rev().copied().collect();
            [rotated, reversed]
        })
        .collect()
}
