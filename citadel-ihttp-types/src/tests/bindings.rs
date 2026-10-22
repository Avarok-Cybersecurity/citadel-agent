//! The TypeScript bindings under `bindings/` are what these types generate.
//!
//! ts-rs writes files and never deletes them, so this generates into a fresh directory and
//! compares both ways: a changed type, a new one and a removed one all fail here. To refresh
//! after changing a type: `CITADEL_WRITE_BINDINGS=1 cargo test -p citadel-ihttp-types
//! --features typescript`, then commit `bindings/`.

use crate::IhttpFrame;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use ts_rs::TS;

fn read_dir(dir: &Path) -> BTreeMap<String, String> {
    std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "ts"))
        .map(|path| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            (name, std::fs::read_to_string(&path).unwrap())
        })
        .collect()
}

#[test]
fn the_committed_bindings_are_what_the_types_generate() {
    let committed_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bindings");
    let fresh_dir =
        std::env::temp_dir().join(format!("citadel-ihttp-bindings-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&fresh_dir);
    IhttpFrame::export_all_to(&fresh_dir).expect("export the bindings");
    let fresh = read_dir(&fresh_dir);
    std::fs::remove_dir_all(&fresh_dir).expect("remove the scratch bindings");
    assert!(fresh.len() >= 5, "the export wrote {} files", fresh.len());
    if std::env::var_os("CITADEL_WRITE_BINDINGS").is_some() {
        let _ = std::fs::remove_dir_all(&committed_dir);
        std::fs::create_dir_all(&committed_dir).unwrap();
        for (name, text) in &fresh {
            std::fs::write(committed_dir.join(name), text).unwrap();
        }
    }
    assert_eq!(
        read_dir(&committed_dir),
        fresh,
        "bindings/ is stale: regenerate with CITADEL_WRITE_BINDINGS=1 and commit it"
    );
}
