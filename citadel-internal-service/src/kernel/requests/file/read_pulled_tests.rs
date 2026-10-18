//! The disk side of `ReadPulledFileChunk`, on a real filesystem (no mocks).

use super::{read_confined, sdk_transfer_root};
use crate::kernel::requests::file::file_identity::FileIdentity;
use crate::kernel::requests::file::upload::MAX_BROWSER_TRANSFER_TOTAL_BYTES;
use citadel_sdk::prelude::BackendType;
use std::path::{Path, PathBuf};

/// An agent home with the SDK's transfer directory, and one received file in it.
fn home() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let home = tempfile::tempdir().unwrap();
    let root = sdk_transfer_root(&BackendType::Filesystem(
        home.path().to_string_lossy().into_owned(),
    ))
    .unwrap();
    std::fs::create_dir_all(root.join("9")).unwrap();
    let file = root.join("9").join("a.txt");
    std::fs::write(&file, b"hello, docs").unwrap();
    (home, root, file)
}

/// The identity of the file `path` reaches, links followed: the escape tests
/// hand over the identity of the very file they reach, so only confinement
/// stops them.
fn id(path: &Path) -> FileIdentity {
    FileIdentity::of(&std::fs::File::open(path).unwrap()).unwrap()
}

#[test]
fn the_root_is_the_sdk_s_transfer_directory() {
    assert_eq!(
        sdk_transfer_root(&BackendType::Filesystem("/data/agent".into())),
        Ok(PathBuf::from("/data/agent/transfers"))
    );
    assert!(sdk_transfer_root(&BackendType::InMemory).is_err());
}

#[test]
fn a_file_inside_the_root_is_read_in_chunks() {
    let (_home, root, file) = home();
    assert_eq!(
        read_confined(&root, &file, &id(&file), 0, 5),
        Ok((11, b"hello".to_vec()))
    );
    assert_eq!(
        read_confined(&root, &file, &id(&file), 5, 1024),
        Ok((11, b", docs".to_vec()))
    );
    assert!(
        read_confined(&root, &file, &id(&file), 11, 5).is_err(),
        "past EOF"
    );
}

#[test]
fn a_traversal_out_of_the_root_is_refused() {
    let (home, root, _file) = home();
    let outside = home.path().join("accounts.secret");
    std::fs::write(&outside, b"keys").unwrap();
    let traversal = root.join("9").join("..").join("..").join("accounts.secret");
    assert!(
        traversal.exists(),
        "the path resolves, so only confinement stops it"
    );
    let refused = read_confined(&root, &traversal, &id(&traversal), 0, 4).unwrap_err();
    assert!(refused.contains("outside"), "{refused}");
}

#[cfg(unix)]
#[test]
fn a_symlink_escaping_the_root_is_refused() {
    let (home, root, _file) = home();
    let outside = home.path().join("accounts.secret");
    std::fs::write(&outside, b"keys").unwrap();
    let link = root.join("9").join("link.txt");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let refused = read_confined(&root, &link, &id(&link), 0, 4).unwrap_err();
    assert!(refused.contains("outside"), "{refused}");
}

#[cfg(unix)]
#[test]
fn a_symlinked_directory_escaping_the_root_is_refused() {
    let (home, root, _file) = home();
    let elsewhere = home.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("b.txt"), b"keys").unwrap();
    std::os::unix::fs::symlink(&elsewhere, root.join("8")).unwrap();
    let reached = root.join("8").join("b.txt");
    assert!(read_confined(&root, &reached, &id(&reached), 0, 4).is_err());
}

#[test]
fn a_directory_is_not_read() {
    let (_home, root, file) = home();
    assert!(read_confined(&root, &root.join("9"), &id(&file), 0, 4).is_err());
}

#[cfg(unix)]
#[test]
fn a_fifo_is_refused_without_hanging() {
    let (_home, root, file) = home();
    let fifo = root.join("9").join("pipe");
    let c_path = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    // SAFETY: a valid NUL-terminated path; mkfifo only creates the node.
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
    let refused = read_confined(&root, &fifo, &id(&file), 0, 4).unwrap_err();
    assert!(refused.contains("not a regular file"), "{refused}");
}

#[test]
fn a_file_that_is_gone_is_refused() {
    let (_home, root, file) = home();
    let gone = root.join("9").join("gone.txt");
    assert!(read_confined(&root, &gone, &id(&file), 0, 4).is_err());
}

#[test]
fn an_oversize_file_is_refused_before_any_bytes() {
    let (_home, root, _file) = home();
    let big = root.join("9").join("big.bin");
    // Sparse: the length is what is checked, so nothing is written to disk.
    std::fs::File::create(&big)
        .unwrap()
        .set_len(MAX_BROWSER_TRANSFER_TOTAL_BYTES + 1)
        .unwrap();
    let refused = read_confined(&root, &big, &id(&big), 0, 1).unwrap_err();
    assert!(refused.contains("over the"), "{refused}");
}

#[test]
fn a_missing_root_refuses_everything() {
    let (_home, _root, file) = home();
    assert!(read_confined(
        Path::new("/nonexistent-citadel-root"),
        &file,
        &id(&file),
        0,
        4
    )
    .is_err());
}

/// Another session pulls a same-named file from the same source: it is written
/// over this session's, in place or as a new file. This session's read of it is
/// refused, rather than handing over the other account's plaintext.
#[test]
fn a_file_replaced_since_the_pull_is_refused() {
    let (_home, root, file) = home();
    let pulled = id(&file);
    std::fs::remove_file(&file).unwrap();
    std::fs::write(&file, b"someone else").unwrap();
    let refused = read_confined(&root, &file, &pulled, 0, 4).unwrap_err();
    assert!(refused.contains("replaced"), "{refused}");
}

#[test]
fn a_file_overwritten_in_place_is_refused() {
    let (_home, root, file) = home();
    let pulled = id(&file);
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    let rewritten = std::fs::OpenOptions::new().write(true).open(&file).unwrap();
    std::io::Write::write_all(&mut &rewritten, b"HELLO, DOCS").unwrap();
    rewritten.set_modified(later).unwrap();
    drop(rewritten);
    assert_eq!(std::fs::read(&file).unwrap().len(), 11, "same length");
    let refused = read_confined(&root, &file, &pulled, 0, 4).unwrap_err();
    assert!(refused.contains("replaced"), "{refused}");
}

/// A directory on the path swapped for another inside the root holding a file
/// of the same name: still confined, but not the file that was pulled.
#[test]
fn a_swapped_directory_opens_a_different_file_and_is_refused() {
    let (_home, root, file) = home();
    let pulled = id(&file);
    std::fs::create_dir(root.join("7")).unwrap();
    std::fs::write(root.join("7").join("a.txt"), b"hello, docs").unwrap();
    std::fs::rename(root.join("9"), root.join("9-old")).unwrap();
    std::fs::rename(root.join("7"), root.join("9")).unwrap();
    let refused = read_confined(&root, &file, &pulled, 0, 4).unwrap_err();
    assert!(refused.contains("replaced"), "{refused}");
}
