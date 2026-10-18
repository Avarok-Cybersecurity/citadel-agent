use super::identify;

#[test]
fn a_regular_file_has_an_identity() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, b"a").unwrap();
    assert!(identify(&file).is_some());
}

#[test]
fn a_directory_or_a_missing_path_has_none() {
    let dir = tempfile::tempdir().unwrap();
    assert!(identify(dir.path()).is_none());
    assert!(identify(&dir.path().join("gone")).is_none());
}

/// A link at the saved path is not the file the SDK wrote, so nothing is recorded.
#[cfg(unix)]
#[test]
fn a_link_has_none_even_to_a_regular_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, b"a").unwrap();
    let link = dir.path().join("link.txt");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    assert!(identify(&link).is_none());
}

/// Same bytes, same length, same modification time: only the file index tells
/// them apart -- which is all Windows has when two files are written back to
/// back, since they share their timestamps there.
#[test]
fn two_files_alike_in_everything_but_index_are_different_files() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("a.txt"), dir.path().join("b.txt"));
    let at = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    for path in [&a, &b] {
        std::fs::write(path, b"same").unwrap();
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(at).unwrap();
    }
    let (ia, ib) = (identify(&a).unwrap(), identify(&b).unwrap());
    assert_ne!(ia, ib);
}
