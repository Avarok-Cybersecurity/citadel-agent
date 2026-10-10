//! Whether a download may replace this agent (pure). Every check has to pass; the first that
//! does not is the reason given, and the install is left as it is. The release's ML-DSA-65
//! signature is checked first; the sha256 and the attestation (and, for the menu-bar app, its
//! code signature) are required as well, not instead.

use super::platform::Method;
use super::version::{is_upgrade, version_line};
use semver::Version;

/// What the release's GitHub attestations said about a download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provenance {
    /// A bundle verified against the Sigstore root, from this repository's release workflow.
    Verified,
    /// The release has none for this file.
    Absent,
    /// There were bundles, and none verified.
    Failed(String),
}

/// What the asset's `.mldsa.sig` said, checked against the embedded release key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseSignature {
    /// By the release key, over this tag, this asset's name and the digest of the download.
    Verified,
    /// The release publishes no `<asset>.mldsa.sig`.
    Missing,
    Refused(String),
}

/// Why a release without `<asset>.mldsa.sig` is refused.
pub const MISSING_SIGNATURE: &str =
    "ML-DSA: the release has no ML-DSA signature for this file, so it is not installed";

/// The signature `signature` (the `.mldsa.sig` file's text, `None` when the release has none)
/// over `asset_name` of `tag`, whose download hashed to `actual_sha` (hex), by `release_key`.
pub fn check_signature(
    release_key: &str,
    tag: &str,
    asset_name: &str,
    actual_sha: &str,
    signature: Option<&str>,
) -> ReleaseSignature {
    let Some(signature) = signature else {
        return ReleaseSignature::Missing;
    };
    let checked = citadel_release_signature::sha256_from_hex(actual_sha).and_then(|digest| {
        citadel_release_signature::verify(release_key, tag, asset_name, &digest, signature)
    });
    match checked {
        Ok(()) => ReleaseSignature::Verified,
        Err(why) => ReleaseSignature::Refused(why.to_string()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Accept,
    /// Not refused, but not to be installed automatically: offer the download.
    LinkOnly(String),
    Refuse(String),
}

/// The digest a `<name>.sha256` asset gives for `name`. Accepts `sha256sum`'s text (`<hex>  name`)
/// and binary (`<hex> *name`) forms, one line per file; the line for `name` must exist.
pub fn expected_sha256(text: &str, name: &str) -> Result<String, String> {
    for line in text.lines() {
        let Some((digest, rest)) = line.split_once(' ') else {
            continue;
        };
        let file = rest.strip_prefix(' ').or_else(|| rest.strip_prefix('*'));
        if file.map(str::trim_end) != Some(name) {
            continue;
        }
        let ok = digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit());
        return if ok {
            Ok(digest.to_ascii_lowercase())
        } else {
            Err(format!(
                "the checksum for {name} is not a sha256: {digest:?}"
            ))
        };
    }
    Err(format!("the checksum file names no {name}"))
}

/// The download, before anything from it is run.
pub fn judge_download(
    current: &Version,
    version: &Version,
    signature: &ReleaseSignature,
    expected_sha: &str,
    actual_sha: &str,
    provenance: &Provenance,
    method: Method,
) -> Verdict {
    match signature {
        ReleaseSignature::Verified => {}
        ReleaseSignature::Missing => return Verdict::Refuse(MISSING_SIGNATURE.to_string()),
        ReleaseSignature::Refused(why) => return Verdict::Refuse(format!("ML-DSA: {why}")),
    }
    if !is_upgrade(current, version) {
        return Verdict::Refuse(format!("{version} would not upgrade {current}"));
    }
    if !expected_sha.eq_ignore_ascii_case(actual_sha) {
        return Verdict::Refuse(format!(
            "the download's sha256 is {actual_sha}, the release says {expected_sha}"
        ));
    }
    match provenance {
        Provenance::Verified => Verdict::Accept,
        Provenance::Failed(why) => {
            Verdict::Refuse(format!("the attestation did not verify: {why}"))
        }
        // The app's own code signature and notarisation are checked before its bundle is swapped
        // (apps/macos-agent); a bare binary or an AppImage has nothing else to vouch for it.
        Provenance::Absent if method == Method::MacApp => Verdict::Accept,
        Provenance::Absent => Verdict::LinkOnly(
            "the release has no attestation for this file, so it is not installed automatically"
                .to_string(),
        ),
    }
}

/// The staged copy: what it printed for `--version`, when it is runnable here (the app's
/// embedded agent is checked by its launcher instead).
pub fn judge_staged(version: &Version, printed: Option<&str>, method: Method) -> Verdict {
    match (printed, method) {
        (None, Method::MacApp) => Verdict::Accept,
        (None, _) => Verdict::Refuse("the staged agent was not run".to_string()),
        (Some(line), _) if line.trim() == version_line(version) => Verdict::Accept,
        (Some(line), _) => Verdict::Refuse(format!(
            "the staged agent says {:?}, the release is {version}",
            line.trim()
        )),
    }
}

#[cfg(test)]
#[path = "verify_tests.rs"]
mod tests;
