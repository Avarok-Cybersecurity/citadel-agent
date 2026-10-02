use super::*;

const A: &str = "5f6950771bbd8acfacd9a11ac35cb40eb5efa98300f35d55691afa78c6187a67";
const B: &str = "4d4ee793f36b1b5a7129187f165a10f6e56c6ec82a019dba9e54aac6c18384ad";

fn v(text: &str) -> Version {
    Version::parse(text).unwrap()
}

#[test]
fn both_sha256sum_forms_are_read_for_the_named_file_only() {
    let text = format!("{B}  other.tar.gz\n{A}  citadel-agent-linux-x64.tar.gz\n");
    assert_eq!(
        expected_sha256(&text, "citadel-agent-linux-x64.tar.gz").unwrap(),
        A
    );
    let text = format!("{B} *Citadel-Agent-x64.msi\r\n");
    assert_eq!(expected_sha256(&text, "Citadel-Agent-x64.msi").unwrap(), B);
}

#[test]
fn a_checksum_for_another_file_or_not_a_sha256_is_refused() {
    assert!(expected_sha256(&format!("{A}  a.dmg"), "Citadel-Agent.dmg").is_err());
    assert!(expected_sha256("abc  Citadel-Agent.dmg", "Citadel-Agent.dmg").is_err());
    assert!(expected_sha256("", "Citadel-Agent.dmg").is_err());
    let nonhex = "z".repeat(64);
    assert!(expected_sha256(&format!("{nonhex}  x"), "x").is_err());
}

#[test]
fn a_matching_attested_upgrade_is_accepted() {
    let verdict = judge_download(
        &v("0.8.8"),
        &v("0.8.9"),
        A,
        &A.to_uppercase(),
        &Provenance::Verified,
        Method::Tarball,
    );
    assert_eq!(verdict, Verdict::Accept);
}

#[test]
fn a_sha_mismatch_is_refused_whatever_else_holds() {
    let verdict = judge_download(
        &v("0.8.8"),
        &v("0.8.9"),
        A,
        B,
        &Provenance::Verified,
        Method::MacApp,
    );
    assert!(matches!(verdict, Verdict::Refuse(why) if why.contains("sha256")));
}

#[test]
fn a_downgrade_or_reinstall_is_refused() {
    for to in ["0.8.7", "0.8.8"] {
        let verdict = judge_download(
            &v("0.8.8"),
            &v(to),
            A,
            A,
            &Provenance::Verified,
            Method::Tarball,
        );
        assert!(matches!(verdict, Verdict::Refuse(_)), "{to}");
    }
}

#[test]
fn a_failed_attestation_is_refused_and_a_missing_one_only_linked_off_the_mac_app() {
    let failed = Provenance::Failed("bad signature".to_string());
    for method in [Method::MacApp, Method::Tarball, Method::AppImage] {
        let verdict = judge_download(&v("0.8.8"), &v("0.8.9"), A, A, &failed, method);
        assert!(matches!(verdict, Verdict::Refuse(_)), "{method:?}");
    }
    let absent =
        |method| judge_download(&v("0.8.8"), &v("0.8.9"), A, A, &Provenance::Absent, method);
    assert_eq!(absent(Method::MacApp), Verdict::Accept);
    assert!(matches!(absent(Method::Tarball), Verdict::LinkOnly(_)));
    assert!(matches!(absent(Method::AppImage), Verdict::LinkOnly(_)));
}

#[test]
fn the_staged_agent_must_print_the_release_version() {
    let ok = judge_staged(&v("0.8.9"), Some("citadel-agent 0.8.9\n"), Method::Tarball);
    assert_eq!(ok, Verdict::Accept);
    for printed in [
        "citadel-agent 0.8.8",
        "citadel-agent 0.8.9-rc.1",
        "0.8.9",
        "",
    ] {
        let verdict = judge_staged(&v("0.8.9"), Some(printed), Method::AppImage);
        assert!(matches!(verdict, Verdict::Refuse(_)), "{printed:?}");
    }
    assert!(matches!(
        judge_staged(&v("0.8.9"), None, Method::Tarball),
        Verdict::Refuse(_)
    ));
    assert_eq!(
        judge_staged(&v("0.8.9"), None, Method::MacApp),
        Verdict::Accept
    );
}
