use super::*;

#[test]
fn a_matching_version_proceeds() {
    assert_eq!(
        verdict(Some(AGENT_DATA_FORMAT), true),
        FormatVerdict::Proceed
    );
}

#[test]
fn a_fresh_directory_is_stamped() {
    assert_eq!(
        verdict(None, false),
        FormatVerdict::Stamp(AGENT_DATA_FORMAT)
    );
}

#[test]
fn the_previous_format_is_refused_as_earlier() {
    assert_eq!(
        verdict(Some(AGENT_DATA_FORMAT - 1), true),
        FormatVerdict::Earlier
    );
    assert_eq!(verdict(Some(1), true), FormatVerdict::Earlier);
}

#[test]
fn data_without_a_marker_is_refused_as_earlier() {
    assert_eq!(verdict(None, true), FormatVerdict::Earlier);
}

#[test]
fn a_newer_format_is_refused() {
    assert_eq!(
        verdict(Some(AGENT_DATA_FORMAT + 1), true),
        FormatVerdict::Newer(AGENT_DATA_FORMAT + 1)
    );
}

#[test]
fn an_earlier_directory_is_refused_on_disk_naming_its_path_and_left_as_it_is() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(FORMAT_MARKER), "1").unwrap();
    std::fs::write(dir.path().join("accounts.db"), b"earlier agent").unwrap();
    let err = check_data_dir(dir.path()).unwrap_err().to_string();
    assert!(err.contains("from an earlier version of Citadel"), "{err}");
    assert!(err.contains("Start fresh by removing"), "{err}");
    assert!(err.contains(&dir.path().display().to_string()), "{err}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join(FORMAT_MARKER)).unwrap(),
        "1"
    );
}

#[test]
fn a_missing_or_empty_directory_is_created_and_stamped() {
    let parent = tempfile::tempdir().unwrap();
    let dir = parent.path().join("agent");
    check_data_dir(&dir).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join(FORMAT_MARKER)).unwrap(),
        AGENT_DATA_FORMAT.to_string()
    );
    check_data_dir(&dir).unwrap();
}

#[test]
fn a_newer_refusal_says_not_to_delete_the_keys() {
    let copy = refusal_copy(&FormatVerdict::Newer(9), Path::new("/x")).unwrap();
    assert!(copy.contains("Do not delete"), "{copy}");
    assert!(copy.contains("INTERNAL_SERVICE_DATA_DIR"), "{copy}");
}
