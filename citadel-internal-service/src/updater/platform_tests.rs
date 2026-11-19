use super::*;

fn facts(os: Os, arch: Arch, exe: &str) -> HostFacts {
    HostFacts {
        os,
        arch,
        exe: PathBuf::from(exe),
        appimage: None,
        dpkg_owned: false,
        launched_by_app: false,
        replaceable: true,
    }
}

fn applied(f: &HostFacts) -> Option<(&'static str, Method)> {
    match plan(&install_kind(f), f.os, f.arch) {
        Plan::Apply { asset, method } => Some((asset, method)),
        Plan::LinkOnly { .. } => None,
    }
}

fn linked(f: &HostFacts) -> Option<&'static str> {
    match plan(&install_kind(f), f.os, f.arch) {
        Plan::LinkOnly { asset, .. } => asset,
        Plan::Apply { .. } => panic!("expected a link for {f:?}"),
    }
}

const APP_EXE: &str = "/Applications/Citadel Agent.app/Contents/MacOS/citadel-agent";

#[test]
fn the_mac_app_started_by_its_launcher_takes_the_disk_image() {
    let mut f = facts(Os::MacOs, Arch::Arm64, APP_EXE);
    f.launched_by_app = true;
    assert_eq!(
        install_kind(&f),
        InstallKind::MacApp {
            bundle: PathBuf::from("/Applications/Citadel Agent.app")
        }
    );
    assert_eq!(applied(&f), Some(("Citadel-Agent.dmg", Method::MacApp)));
}

#[test]
fn the_mac_app_binary_run_by_hand_is_only_offered_a_link() {
    let f = facts(Os::MacOs, Arch::X64, APP_EXE);
    assert!(matches!(install_kind(&f), InstallKind::Unknown { .. }));
    assert_eq!(linked(&f), None);
}

#[test]
fn a_tarball_binary_takes_the_tarball_for_its_processor() {
    let f = facts(Os::MacOs, Arch::X64, "/Users/a/bin/citadel-agent");
    assert_eq!(
        applied(&f),
        Some(("citadel-agent-macos-x64.tar.gz", Method::Tarball))
    );
    let f = facts(Os::MacOs, Arch::Arm64, "/Users/a/bin/citadel-agent");
    assert_eq!(
        applied(&f),
        Some(("citadel-agent-macos-arm64.tar.gz", Method::Tarball))
    );
    let f = facts(Os::Linux, Arch::X64, "/home/a/citadel-agent");
    assert_eq!(
        applied(&f),
        Some(("citadel-agent-linux-x64.tar.gz", Method::Tarball))
    );
}

#[test]
fn linux_arm64_has_no_release_so_gets_the_release_page() {
    let f = facts(Os::Linux, Arch::Arm64, "/home/a/citadel-agent");
    assert_eq!(linked(&f), None);
}

#[test]
fn an_appimage_takes_the_appimage() {
    let mut f = facts(
        Os::Linux,
        Arch::X64,
        "/tmp/.mount_abc/usr/bin/citadel-agent",
    );
    f.appimage = Some(PathBuf::from("/home/a/Citadel-Agent-x86_64.AppImage"));
    assert_eq!(
        install_kind(&f),
        InstallKind::AppImage {
            image: PathBuf::from("/home/a/Citadel-Agent-x86_64.AppImage")
        }
    );
    assert_eq!(
        applied(&f),
        Some(("Citadel-Agent-x86_64.AppImage", Method::AppImage))
    );
}

#[test]
fn a_dpkg_install_is_linked_to_the_deb_even_as_an_appimage_variable_leaks_in() {
    let mut f = facts(Os::Linux, Arch::X64, "/usr/bin/citadel-agent");
    f.dpkg_owned = true;
    f.appimage = Some(PathBuf::from("/home/a/x.AppImage"));
    assert_eq!(install_kind(&f), InstallKind::Deb);
    assert_eq!(linked(&f), Some("citadel-agent-linux-x64.deb"));
}

#[test]
fn windows_is_linked_to_the_msi() {
    let f = facts(
        Os::Windows,
        Arch::X64,
        r"C:\Program Files\Citadel Agent\citadel-agent.exe",
    );
    assert_eq!(linked(&f), Some("Citadel-Agent-x64.msi"));
}

#[test]
fn an_unwritable_location_is_never_applied() {
    let mut f = facts(Os::Linux, Arch::X64, "/opt/citadel/citadel-agent");
    f.replaceable = false;
    assert_eq!(linked(&f), None);
    let mut f = facts(Os::MacOs, Arch::Arm64, "/usr/local/bin/citadel-agent");
    f.replaceable = false;
    assert_eq!(linked(&f), None);
    let mut f = facts(Os::Linux, Arch::X64, "/tmp/.mount/citadel-agent");
    f.appimage = Some(PathBuf::from("/opt/x.AppImage"));
    f.replaceable = false;
    assert_eq!(linked(&f), None);
}

#[test]
fn a_path_that_only_looks_like_a_bundle_is_not_one() {
    assert_eq!(
        mac_bundle_of(std::path::Path::new("/a/Foo.app/MacOS/citadel-agent")),
        None
    );
    assert_eq!(
        mac_bundle_of(std::path::Path::new("/a/Foo/Contents/MacOS/citadel-agent")),
        None
    );
}

/// Which installs replace themselves, decided before any download: the mac app and an
/// AppImage or tarball in a writable place do; dpkg, the per-machine MSI and the rest say why
/// not, so the window offers Download instead of a Restart that cannot finish.
#[test]
fn whether_an_install_updates_itself_is_known_up_front_with_the_reason() {
    let can = |f: &HostFacts| self_update(&plan(&install_kind(f), f.os, f.arch));
    let mut mac = facts(Os::MacOs, Arch::Arm64, APP_EXE);
    mac.launched_by_app = true;
    let mut image = facts(Os::Linux, Arch::X64, "/tmp/.mount_x/usr/bin/citadel-agent");
    image.appimage = Some(PathBuf::from("/home/a/Citadel-Agent-x86_64.AppImage"));
    for f in [
        &mac,
        &image,
        &facts(Os::Linux, Arch::X64, "/home/a/citadel-agent"),
    ] {
        assert_eq!(can(f), Ok(()), "{f:?}");
    }
    let mut deb = facts(Os::Linux, Arch::X64, "/usr/bin/citadel-agent");
    deb.dpkg_owned = true;
    let msi = facts(
        Os::Windows,
        Arch::X64,
        r"C:\Program Files\Citadel Agent\citadel-agent.exe",
    );
    let by_hand = facts(Os::MacOs, Arch::Arm64, APP_EXE);
    for (f, said) in [
        (&deb, "package manager"),
        (&msi, "administrator"),
        (&by_hand, "not started by it"),
    ] {
        let why = can(f).expect_err("not self-updating");
        assert!(why.contains(said), "{f:?}: {why}");
    }
}
