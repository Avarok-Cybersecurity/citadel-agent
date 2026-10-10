//! The updater's ML-DSA-65 release signature, end to end: the engine over the fake release server
//! of tests/updater_engine.rs, whose every asset is signed with a fixture key that reaches the
//! engine through `Io::release_key`, as the shipped key does (kernel/updates.rs).
//!
//! Mocks, and why: as in tests/updater_engine.rs. The signature check itself is the production
//! code, with a real ML-DSA-65 key; only which key is trusted differs.

#[allow(dead_code)]
#[path = "updater_fakes/mod.rs"]
mod fakes;

use citadel_internal_service::updater::platform::{Method, Plan};
use citadel_internal_service::updater::policy::Trigger;
use citadel_release_signature::{ReleaseSigningKey, PLACEHOLDER_PUBLIC_KEY};
use fakes::*;

const TAG: &str = "agent-v0.9.0";
const TARBALL: &str = "citadel-agent-linux-x64.tar.gz";
const DMG: &str = "Citadel-Agent.dmg";

fn plan(asset: &'static str, method: Method) -> Plan {
    Plan::Apply { asset, method }
}

fn signed_world(asset: &'static str, method: Method) -> World {
    World::new(TAG, plan(asset, method), "citadel-agent 0.9.0")
}

/// After the check: not ready, not verified, nothing installed even when asked, and the
/// reason is the signature's.
async fn assert_refused(world: &World, reason: &str) {
    let update = world.announced().pop().expect("an update is announced");
    assert!(!update.ready && !update.mldsa_verified, "{update:?}");
    assert_eq!(
        update.download_url, update.notes_url,
        "a refused file is not linked"
    );
    let why = world.engine.status(None).last_error.unwrap();
    assert!(why.starts_with("ML-DSA:") && why.contains(reason), "{why}");
    assert_eq!(world.source.attestation_queries(), 0, "nothing else ran");
    world.sessions.set(0);
    assert!(world.engine.consider(Trigger::UserAsked).await.is_err());
    assert!(world.installed().is_empty());
    assert!(!world.staged_dir_exists("0.9.0"));
}

#[tokio::test]
async fn a_signed_release_is_staged_reported_verified_and_installed() {
    for (asset, method) in [(TARBALL, Method::Tarball), (DMG, Method::MacApp)] {
        let world = signed_world(asset, method);
        world.engine.check().await;
        let update = &world.announced()[0];
        assert!(update.ready && update.mldsa_verified, "{asset}: {update:?}");
        assert_eq!(world.engine.status(None).last_error, None);
        world.engine.consider(Trigger::Idle).await.unwrap();
        assert_eq!(world.installed().len(), 1, "{asset}");
    }
}

#[tokio::test]
async fn a_release_without_a_signature_is_refused_before_anything_is_downloaded() {
    for (asset, method) in [(TARBALL, Method::Tarball), (DMG, Method::MacApp)] {
        let mut world = signed_world(asset, method);
        world.source.drop_asset(&format!("{asset}.mldsa.sig"));
        world.engine = world.rebuild();
        world.engine.check().await;
        assert_refused(&world, "no ML-DSA signature").await;
        assert_eq!(world.source.downloads(), 0, "{asset}");
    }
}

#[tokio::test]
async fn a_tampered_asset_is_refused_by_its_signature_first() {
    let mut world = signed_world(TARBALL, Method::Tarball);
    world.source.tamper(TARBALL);
    world.engine = world.rebuild();
    world.engine.check().await;
    assert_refused(&world, "does not verify").await;
}

#[tokio::test]
async fn a_signature_for_another_tag_or_another_asset_is_refused() {
    // The download's own bytes, truly signed: only the tag or the name is another's.
    for (tag, name) in [("agent-v0.8.9", TARBALL), (TAG, DMG)] {
        let mut world = signed_world(TARBALL, Method::Tarball);
        let moved = signature(&RELEASE_SEED, tag, name, &world.source.bytes(TARBALL));
        world.source.set_signature(TARBALL, &moved);
        world.engine = world.rebuild();
        world.engine.check().await;
        assert_refused(&world, "does not verify").await;
    }
}

#[tokio::test]
async fn another_keys_signature_is_refused_and_verifies_under_that_key() {
    let other = [0x11; 32];
    let mut world = signed_world(TARBALL, Method::Tarball);
    let forged = signature(&other, TAG, TARBALL, &world.source.bytes(TARBALL));
    world.source.set_signature(TARBALL, &forged);
    world.engine = world.rebuild();
    world.engine.check().await;
    assert_refused(&world, "does not verify").await;

    // Negative control: trusting that key instead, its signature is accepted. (Signed afresh:
    // each world's tarball carries its own mtimes, so its bytes are its own.)
    let mut world = signed_world(TARBALL, Method::Tarball);
    let forged = signature(&other, TAG, TARBALL, &world.source.bytes(TARBALL));
    world.source.set_signature(TARBALL, &forged);
    world.release_key = ReleaseSigningKey::from_seed(&other).public_key_hex();
    world.engine = world.rebuild();
    world.engine.check().await;
    let update = &world.announced()[0];
    let status = world.engine.status(None);
    assert!(update.ready && update.mldsa_verified, "{status:?}");
}

#[tokio::test]
async fn a_truncated_or_empty_signature_is_refused() {
    for half in [true, false] {
        let mut world = signed_world(TARBALL, Method::Tarball);
        let good = signature(&RELEASE_SEED, TAG, TARBALL, &world.source.bytes(TARBALL));
        let bad = if half { &good[..good.len() / 2] } else { "" };
        world.source.set_signature(TARBALL, bad);
        world.engine = world.rebuild();
        world.engine.check().await;
        assert_refused(&world, "malformed").await;
    }
}

#[tokio::test]
async fn the_placeholder_key_refuses_a_correctly_signed_release() {
    let mut world = signed_world(TARBALL, Method::Tarball);
    world.release_key = PLACEHOLDER_PUBLIC_KEY.to_string();
    world.engine = world.rebuild();
    world.engine.check().await;
    assert_refused(&world, "placeholder").await;
}
