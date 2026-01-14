#[cfg(feature = "typescript")]
fn main() {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    // Get git repository root directory
    let git_root_output = Command::new("git")
        .args(&["rev-parse", "--show-toplevel"])
        .output()
        .expect("Failed to execute git command");

    if git_root_output.status.success() {
        let git_root = String::from_utf8(git_root_output.stdout)
            .expect("Invalid UTF-8 in git root path")
            .trim()
            .to_string();

        let typescript_types_path = Path::new(&git_root).join("typescript-client/src/types");

        // Create TypeScript types directory
        fs::create_dir_all(&typescript_types_path).unwrap();

        // Set TS_RS_EXPORT_DIR environment variable for ts-rs at compile time
        println!(
            "cargo:rustc-env=TS_RS_EXPORT_DIR={}",
            typescript_types_path.display()
        );

        println!(
            "cargo:debug=TypeScript types will be automatically generated to: {:?}",
            typescript_types_path
        );

        // After the main crate is built, automatically run the TypeScript generation
        // This ensures all types are exported with proper imports
        println!("cargo:warning=To complete TypeScript type generation, run: TS_RS_EXPORT_DIR={} cargo run --example generate_ts_types --features typescript", typescript_types_path.display());
    } else {
        println!("cargo:warning=Could not determine git root, TypeScript types may not be generated correctly");
    }

    // Tell Cargo to re-run this build script when these files change
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=examples/generate_ts_types.rs");
}

#[cfg(not(feature = "typescript"))]
fn main() {
    // Do nothing when typescript feature is not enabled
}
