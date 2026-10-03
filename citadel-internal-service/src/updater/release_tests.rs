use super::*;

fn release(tag: &str) -> Release {
    Release {
        tag_name: tag.to_string(),
        draft: false,
        prerelease: false,
        assets: vec![Asset {
            name: "Citadel-Agent.dmg".to_string(),
            browser_download_url: format!("{DOWNLOAD_PREFIX}{tag}/Citadel-Agent.dmg"),
            size: 10,
        }],
    }
}

fn v(text: &str) -> Version {
    Version::parse(text).unwrap()
}

#[test]
fn a_newer_stable_release_is_an_upgrade() {
    assert_eq!(
        candidate(&release("agent-v0.8.9"), &v("0.8.8")),
        Candidate::Upgrade(v("0.8.9"))
    );
}

#[test]
fn drafts_prereleases_downgrades_and_other_tags_are_not() {
    let mut draft = release("agent-v0.9.0");
    draft.draft = true;
    let mut pre = release("agent-v0.9.0");
    pre.prerelease = true;
    for r in [
        draft,
        pre,
        release("agent-v0.8.7"),
        release("agent-v0.8.8"),
        release("v0.9.0"),
    ] {
        assert!(
            matches!(candidate(&r, &v("0.8.8")), Candidate::None(_)),
            "{r:?}"
        );
    }
}

#[test]
fn the_real_latest_response_parses() {
    let body = include_bytes!("../../tests/fixtures/updater/latest-0.8.6.json");
    let r = parse(body).unwrap();
    assert_eq!(r.tag_name, "agent-v0.8.6");
    assert!(asset(&r, "Citadel-Agent.dmg").is_ok());
    assert!(asset(&r, "Citadel-Agent.dmg.sha256").is_ok());
}

#[test]
fn an_asset_pointing_anywhere_but_its_release_is_refused() {
    let mut r = release("agent-v0.9.0");
    r.assets[0].browser_download_url =
        "https://github.com/someone-else/fork/releases/download/agent-v0.9.0/Citadel-Agent.dmg"
            .to_string();
    assert!(asset(&r, "Citadel-Agent.dmg").is_err());
    let mut r = release("agent-v0.9.0");
    r.assets[0].browser_download_url = format!("{DOWNLOAD_PREFIX}agent-v0.8.0/Citadel-Agent.dmg");
    assert!(asset(&r, "Citadel-Agent.dmg").is_err());
    assert!(asset(&release("agent-v0.9.0"), "missing.msi").is_err());
}

#[test]
fn only_https_to_github_hosts_is_allowed() {
    let ok = |u: &str| url_allowed(&reqwest::Url::parse(u).unwrap()).is_ok();
    assert!(ok("https://api.github.com/repos/x"));
    assert!(ok("https://release-assets.githubusercontent.com/a?b=c"));
    assert!(ok("https://objects.githubusercontent.com/a"));
    assert!(ok("https://github.com/a"));
    assert!(!ok("http://github.com/a"));
    assert!(!ok("https://github.com.evil.example/a"));
    assert!(!ok("https://evil.example/github.com"));
    assert!(!ok("https://user:pw@github.com/a"));
    assert!(!ok("https://github.com:8443/a"));
    assert!(!ok("https://gist.githubusercontent.com/a"));
}

#[test]
fn the_notes_url_is_built_from_the_tag() {
    assert_eq!(
        notes_url(&release("agent-v0.9.0")),
        "https://github.com/Avarok-Cybersecurity/citadel-workspace/releases/tag/agent-v0.9.0"
    );
}
