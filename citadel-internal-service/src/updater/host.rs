//! The facts the install type is decided from (platform.rs), read from this machine.

use super::platform::{Arch, HostFacts, Os};
use std::path::{Path, PathBuf};

/// dpkg's record of the files the citadel-agent package installed.
const DPKG_LIST: &str = "/var/lib/dpkg/info/citadel-agent.list";
pub const APPIMAGE_ENV: &str = "APPIMAGE";

/// `launched_by_app`: the menu-bar app's launch token is in this agent's environment.
pub fn gather(launched_by_app: bool) -> Result<HostFacts, String> {
    let exe = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .map_err(|e| format!("the running executable could not be located: {e}"))?;
    let appimage = std::env::var_os(APPIMAGE_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let dpkg_owned = std::fs::read_to_string(DPKG_LIST)
        .map(|list| lists(&list, &exe))
        .unwrap_or(false);
    let replaced = appimage.clone().unwrap_or_else(|| exe.clone());
    Ok(HostFacts {
        os: os(),
        arch: arch(),
        replaceable: can_create_beside(&replaced),
        exe,
        appimage,
        dpkg_owned,
        launched_by_app,
    })
}

/// Whether dpkg's file `list` names `exe` (or the path `exe` resolves from, /usr/bin/...).
pub fn lists(list: &str, exe: &Path) -> bool {
    list.lines().any(|line| {
        let listed = Path::new(line.trim());
        listed == exe || std::fs::canonicalize(listed).is_ok_and(|p| p == exe)
    })
}

/// Whether this user may create a file beside `path`, so a swap there can be a rename. Tried,
/// not inferred from mode bits, which say nothing about ACLs, read-only mounts or sandboxes.
fn can_create_beside(path: &Path) -> bool {
    let Some(dir) = path.parent() else {
        return false;
    };
    tempfile::Builder::new()
        .prefix(".citadel-agent-probe")
        .tempfile_in(dir)
        .is_ok()
}

fn os() -> Os {
    match std::env::consts::OS {
        "macos" => Os::MacOs,
        "linux" => Os::Linux,
        "windows" => Os::Windows,
        _ => Os::Other,
    }
}

fn arch() -> Arch {
    match std::env::consts::ARCH {
        "aarch64" => Arch::Arm64,
        "x86_64" => Arch::X64,
        _ => Arch::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpkg_lists_name_the_executable_exactly() {
        let list = "/.\n/usr\n/usr/bin\n/usr/bin/citadel-agent\n/usr/bin/citadel-agent-launch\n";
        assert!(lists(list, Path::new("/usr/bin/citadel-agent")));
        assert!(!lists(list, Path::new("/home/a/citadel-agent")));
        assert!(!lists(list, Path::new("/usr/bin/citadel")));
    }

    #[test]
    fn a_writable_directory_is_replaceable_and_a_missing_one_is_not() {
        let dir = tempfile::tempdir().unwrap();
        assert!(can_create_beside(&dir.path().join("citadel-agent")));
        assert!(!can_create_beside(&dir.path().join("gone/citadel-agent")));
    }
}
