//! The real staging directory, in a temporary root. The "agent" in each archive is a two-line
//! shell script, so `--version` is a real process run, not a stand-in.

use super::*;

fn v(text: &str) -> Version {
    Version::parse(text).unwrap()
}

/// A release-shaped tarball (`./citadel-agent`, `./README.md`) whose agent prints `prints`.
fn tarball(dir: &Path, prints: &str) -> PathBuf {
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join(BINARY), format!("#!/bin/sh\necho '{prints}'\n")).unwrap();
    std::fs::write(src.join("README.md"), "readme").unwrap();
    let out = dir.join("agent.tar.gz");
    let status = std::process::Command::new("tar")
        .arg("-czf")
        .arg(&out)
        .arg("-C")
        .arg(&src)
        .args(["./citadel-agent", "./README.md"])
        .status()
        .unwrap();
    assert!(status.success());
    out
}

#[tokio::test]
async fn the_sha256_is_the_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f");
    std::fs::write(&path, b"abc").unwrap();
    let staging = FsStaging::new(dir.path().join("root"));
    assert_eq!(
        staging.sha256(&path).await.unwrap(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_tarball_stages_its_binary_alone_and_it_runs() {
    let dir = tempfile::tempdir().unwrap();
    let archive = tarball(dir.path(), "citadel-agent 0.9.0");
    let staging = FsStaging::new(dir.path().join("root"));
    let download = staging.download_path(&v("0.9.0"), "agent.tar.gz").unwrap();
    std::fs::copy(&archive, &download).unwrap();
    let binary = staging.stage(Method::Tarball, &download).await.unwrap();
    assert_eq!(binary.file_name().unwrap(), BINARY);
    assert!(!binary.with_file_name("README.md").exists());
    assert_eq!(
        staging.version_of(&binary).await.unwrap().trim(),
        "citadel-agent 0.9.0"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn an_archive_without_the_binary_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("other"), "x").unwrap();
    let archive = dir.path().join("a.tar.gz");
    std::process::Command::new("tar")
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(&src)
        .arg("./other")
        .status()
        .unwrap();
    let staging = FsStaging::new(dir.path().join("root"));
    assert!(staging.stage(Method::Tarball, &archive).await.is_err());
}

#[test]
fn a_new_version_clears_the_old_ones_and_failures_are_remembered() {
    let dir = tempfile::tempdir().unwrap();
    let staging = FsStaging::new(dir.path().to_path_buf());
    let old = staging.download_path(&v("0.9.0"), "a").unwrap();
    std::fs::write(&old, b"x").unwrap();
    let new = staging.download_path(&v("0.9.1"), "a").unwrap();
    assert!(!old.exists() && new.parent().unwrap().exists());
    assert!(!staging.failed(&v("0.9.1")));
    staging.mark_failed(&v("0.9.1"));
    assert!(staging.failed(&v("0.9.1")));
    assert!(!staging.failed(&v("0.9.2")));
    staging.download_path(&v("0.9.2"), "a").unwrap();
    assert!(staging.failed(&v("0.9.1")), "a marker is not a download");
    staging.discard(&v("0.9.2"));
    assert!(!dir.path().join("0.9.2").exists());
}
