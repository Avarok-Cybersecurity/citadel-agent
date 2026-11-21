//! The release's launch settings (launch_settings.rs), fetched and verified. Refused unless
//! the release carries the asset and its `.mldsa.sig`, the signature is the release key's over
//! this tag, this name and these exact bytes, and the bytes are a format this agent reads.

use super::engine::Engine;
use super::fetch::{Refusal, MAX_SIGNATURE_BYTES};
use super::launch_settings::{self, LaunchSettings, ASSET_NAME, MAX_BYTES};
use super::release::{self, Release};
use super::verify::{check_signature, ReleaseSignature};
use citadel_release_signature::SIGNATURE_SUFFIX;

impl Engine {
    pub(super) async fn launch_settings(
        &self,
        release: &Release,
    ) -> Result<LaunchSettings, Refusal> {
        let refused = Refusal::Refused;
        let signature_name = format!("{ASSET_NAME}{SIGNATURE_SUFFIX}");
        let listed = |name: &str| release.assets.iter().any(|a| a.name == name);
        if !listed(ASSET_NAME) {
            return Err(refused(launch_settings::missing(ASSET_NAME)));
        }
        if !listed(&signature_name) {
            return Err(refused(launch_settings::missing(&signature_name)));
        }
        let asset = release::asset(release, ASSET_NAME).map_err(refused)?;
        let signature = release::asset(release, &signature_name).map_err(refused)?;
        if asset.size > MAX_BYTES {
            return Err(refused(format!(
                "{ASSET_NAME} is {} bytes, over the {MAX_BYTES} the updater accepts",
                asset.size
            )));
        }
        let transient = Refusal::Transient;
        let source = &self.io.source;
        let text = source
            .text(&asset.browser_download_url, MAX_BYTES)
            .await
            .map_err(transient)?;
        let signature = source
            .text(&signature.browser_download_url, MAX_SIGNATURE_BYTES)
            .await
            .map_err(transient)?;
        let digest = hex::encode(citadel_release_signature::sha256(text.as_bytes()));
        let key = &self.io.release_key;
        match check_signature(
            key,
            &release.tag_name,
            ASSET_NAME,
            &digest,
            Some(&signature),
        ) {
            ReleaseSignature::Verified => launch_settings::parse(text.as_bytes()).map_err(refused),
            ReleaseSignature::Missing => Err(refused(launch_settings::missing(&signature_name))),
            ReleaseSignature::Refused(why) => Err(refused(format!("ML-DSA: {ASSET_NAME}: {why}"))),
        }
    }
}
