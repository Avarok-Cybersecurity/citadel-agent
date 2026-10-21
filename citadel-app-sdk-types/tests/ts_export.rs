//! Writes `bindings/AbiConstants.ts` beside ts-rs's own exports (`cargo test --features ts`);
//! CI regenerates and diffs, like every other generated binding here.
#![cfg(feature = "ts")]

#[test]
fn export_abi_constants() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("bindings");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("AbiConstants.ts"),
        citadel_app_sdk_types::render_ts_constants(),
    )
    .unwrap();
}
