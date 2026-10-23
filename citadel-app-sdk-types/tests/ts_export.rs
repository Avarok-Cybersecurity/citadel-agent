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

/// The host admits a call when the install holds ANY ONE of `granted_by`; an any-of row is
/// written out literally, since the TypeScript host is built from this line.
#[test]
fn the_any_of_row_renders_as_the_host_reads_it() {
    let ts = citadel_app_sdk_types::render_ts_constants();
    let row = r#"  { group: "ihttp", op: 4, name: "IhttpConsumed", granted_by: ["ihttp.serve", "ihttp.call", "ihttp.p2p.serve", "ihttp.p2p.call"] },"#;
    assert!(ts.lines().any(|l| l == row), "{ts}");
    assert!(ts.contains("granted_by: readonly Capability[] | null }[];"));
    assert!(
        !ts.contains("capability:"),
        "the single-capability field is gone"
    );
}

/// The plane checks the host makes after admitting an ihttp op, as the host reads them.
#[test]
fn the_plane_tables_render_as_the_host_reads_them() {
    let ts = citadel_app_sdk_types::render_ts_constants();
    for line in [
        r#"  p2p: { first: "ihttp.p2p.call", fallback: null },"#,
        r#"  relay: { first: "ihttp.call", fallback: null },"#,
        r#"  prefer_p2p_then_relay: { first: "ihttp.p2p.call", fallback: "ihttp.call" },"#,
        r#"  relay: "ihttp.serve","#,
        r#"  p2p: "ihttp.p2p.serve","#,
        r#"export const LIMIT_TOPIC_RATE = "topic_rate" as const;"#,
        r#"export const MAX_TOPICS = 4 as const;"#,
    ] {
        assert!(ts.lines().any(|l| l == line), "missing {line}\n{ts}");
    }
}
