//! How this agent was installed, and which release asset replaces it (pure).
//!
//! The install type is decided from facts gathered at start (host.rs), never guessed: when the
//! facts do not say how to replace this agent safely, the answer is a download link.

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    MacOs,
    Linux,
    Windows,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    Arm64,
    X64,
    Other,
}

/// What `host.rs` found about the running agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostFacts {
    pub os: Os,
    pub arch: Arch,
    /// The running executable, symlinks resolved.
    pub exe: PathBuf,
    /// `$APPIMAGE`, set by the AppImage runtime to the image the agent runs from.
    pub appimage: Option<PathBuf>,
    /// The executable is listed in dpkg's file list for the citadel-agent package.
    pub dpkg_owned: bool,
    /// The menu-bar app started this agent: its launch token is in the environment.
    pub launched_by_app: bool,
    /// A file can be created beside the thing that would be replaced (the executable, or the
    /// AppImage), so a swap there can be a rename.
    pub replaceable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallKind {
    /// `Citadel Agent.app`, run by its launcher, which swaps the bundle.
    MacApp {
        bundle: PathBuf,
    },
    /// A bare executable from a release tarball, in a directory this user can write.
    Binary {
        exe: PathBuf,
    },
    AppImage {
        image: PathBuf,
    },
    /// Installed by dpkg: only the package manager, as root, may replace it.
    Deb,
    /// Installed by the per-machine MSI: replacing it needs elevation.
    Msi,
    Unknown {
        why: String,
    },
}

/// How a verified download becomes the running agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// Handed to the menu-bar app (the .dmg).
    MacApp,
    /// The tarball's `citadel-agent` replaces the executable.
    Tarball,
    /// The AppImage replaces the AppImage.
    AppImage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    Apply {
        asset: &'static str,
        method: Method,
    },
    /// Offer `asset` (or, with none, the release page) to download by hand, and say why.
    LinkOnly {
        asset: Option<&'static str>,
        why: String,
    },
}

pub fn install_kind(f: &HostFacts) -> InstallKind {
    let in_bundle = mac_bundle_of(&f.exe);
    match f.os {
        Os::MacOs => match (in_bundle, f.launched_by_app) {
            (Some(bundle), true) => InstallKind::MacApp { bundle },
            (Some(_), false) => InstallKind::Unknown {
                why: "inside Citadel Agent.app but not started by it".to_string(),
            },
            (None, _) if f.replaceable => InstallKind::Binary { exe: f.exe.clone() },
            (None, _) => unwritable(f),
        },
        Os::Linux if f.dpkg_owned => InstallKind::Deb,
        Os::Linux => match &f.appimage {
            Some(image) if f.replaceable => InstallKind::AppImage {
                image: image.clone(),
            },
            Some(_) => unwritable(f),
            None if f.replaceable => InstallKind::Binary { exe: f.exe.clone() },
            None => unwritable(f),
        },
        Os::Windows => InstallKind::Msi,
        Os::Other => InstallKind::Unknown {
            why: "no release is built for this operating system".to_string(),
        },
    }
}

pub fn plan(kind: &InstallKind, os: Os, arch: Arch) -> Plan {
    let tarball = match (os, arch) {
        (Os::MacOs, Arch::Arm64) => Some("citadel-agent-macos-arm64.tar.gz"),
        (Os::MacOs, Arch::X64) => Some("citadel-agent-macos-x64.tar.gz"),
        (Os::Linux, Arch::X64) => Some("citadel-agent-linux-x64.tar.gz"),
        _ => None,
    };
    let link = |asset: Option<&'static str>, why: &str| Plan::LinkOnly {
        asset,
        why: why.to_string(),
    };
    match kind {
        InstallKind::MacApp { .. } => Plan::Apply {
            asset: "Citadel-Agent.dmg",
            method: Method::MacApp,
        },
        InstallKind::Binary { .. } => match tarball {
            Some(asset) => Plan::Apply {
                asset,
                method: Method::Tarball,
            },
            None => link(None, "no release is built for this processor"),
        },
        InstallKind::AppImage { .. } if arch == Arch::X64 => Plan::Apply {
            asset: "Citadel-Agent-x86_64.AppImage",
            method: Method::AppImage,
        },
        InstallKind::AppImage { .. } => link(None, "no AppImage is built for this processor"),
        InstallKind::Deb if arch == Arch::X64 => link(
            Some("citadel-agent-linux-x64.deb"),
            "installed with dpkg: the package manager installs the update",
        ),
        InstallKind::Deb => link(None, "no .deb is built for this processor"),
        InstallKind::Msi => link(
            Some("Citadel-Agent-x64.msi"),
            "the Windows installer needs administrator approval",
        ),
        InstallKind::Unknown { why } => link(None, why),
    }
}

/// `…/Name.app` when `exe` is `…/Name.app/Contents/MacOS/<file>`.
fn mac_bundle_of(exe: &std::path::Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    let named = |p: &std::path::Path, name: &str| p.file_name().is_some_and(|n| n == name);
    let is_app = bundle.extension().is_some_and(|e| e == "app");
    (named(macos, "MacOS") && named(contents, "Contents") && is_app).then(|| bundle.to_path_buf())
}

fn unwritable(f: &HostFacts) -> InstallKind {
    InstallKind::Unknown {
        why: format!("{} cannot be replaced by this user", f.exe.display()),
    }
}

#[cfg(test)]
#[path = "platform_tests.rs"]
mod tests;
