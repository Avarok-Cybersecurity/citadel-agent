//! The pinned app servers: one validation, the dial apart from the domain, and a held
//! account signed in to again only as the configuration would register it.
//!
//! Controls, each applied, seen red and reverted:
//! - drop the canonical comparison of the domain in `validated` →
//!   `each_invalid_kind_is_refused_naming_the_entry` goes red.
//! - make `discovery` dial the canonical domain (`dial: domain.to_string()`) →
//!   `a_pinned_plan_dials_the_pin_and_registers_under_the_domain` goes red.
//! - answer `Ok(())` in `sign_in_again`'s `(None, false)` arm →
//!   `a_pin_removed_after_registration_is_refused` goes red.
//! - drop the `dial != held` comparison in `sign_in_again` →
//!   `a_changed_pin_is_refused_and_an_unchanged_one_signs_in` goes red.

use super::*;
use crate::kernel::identity::plan;
use citadel_identity::{fingerprint, ServerKey};

const DOMAIN: &str = "synchronized.work";
const DIAL: &str = "127.0.0.1:18790";

fn fp_of(seed: u8) -> String {
    fingerprint(ServerKey::from_seed(&[seed; 32]).public_key())
}

fn entry(domain: &str, fp: &str, dial: &str) -> PinEntry {
    PinEntry {
        domain: domain.to_string(),
        fingerprint: fp.to_string(),
        dial: dial.to_string(),
    }
}

fn config() -> AppSessionConfig {
    AppSessionConfig::new([entry(DOMAIN, &fp_of(1), DIAL)]).unwrap()
}

/// The account's record as a registration through `discovery` writes it.
fn record(fp: &str, discovered: bool) -> Pin {
    Pin {
        handle: "abc.app-0f3a9c2e".to_string(),
        domain: canonical_domain(DOMAIN).unwrap(),
        server_fp: fp.to_string(),
        server_pk: None,
        issued_at_s: discovered.then_some(1_794_700_000),
        id_fp: "3foriftckvpoy2rh".to_string(),
    }
}

#[test]
fn valid_pins_are_held_in_their_one_form() {
    let fp = fp_of(1);
    let config = AppSessionConfig::new([
        entry(DOMAIN, &fp, DIAL),
        entry("localhost:18791", &fp, "localhost:18791"),
        entry("a.example", &fp, "ws://localhost:8787/acme"),
        entry("b.example", &fp, "wss://b.example/citadel"),
        entry("c.example", &fp, "[::1]:12400"),
    ])
    .unwrap();
    let pin = config.pinned(DOMAIN).unwrap();
    assert_eq!((pin.fingerprint(), pin.dial()), (fp.as_str(), DIAL));
    assert_eq!(
        config.pinned("a.example").unwrap().dial(),
        "ws://localhost:8787/acme"
    );
    assert_eq!(config.pins().count(), 5);
    assert!(AppSessionConfig::new(Vec::new())
        .unwrap()
        .pinned(DOMAIN)
        .is_none());
}

#[test]
fn each_invalid_kind_is_refused_naming_the_entry() {
    let fp = fp_of(1);
    let dashed = format!("{}-{}-{}-{}", &fp[..4], &fp[4..8], &fp[8..12], &fp[12..]);
    let upper = fp.to_ascii_uppercase();
    let cases = [
        (entry("Synchronized.Work", &fp, DIAL), "not canonical"),
        (entry("synchronized.work:443", &fp, DIAL), "not canonical"),
        (
            entry("https://synchronized.work", &fp, DIAL),
            "not canonical",
        ),
        (entry(" synchronized.work", &fp, DIAL), "synchronized.work"),
        (entry(DOMAIN, &upper, DIAL), "fingerprint"),
        (entry(DOMAIN, &dashed, DIAL), "fingerprint"),
        (entry(DOMAIN, &fp[..15], DIAL), "fingerprint"),
        (entry(DOMAIN, &fp, DOMAIN), "names no port"),
        (entry(DOMAIN, &fp, "Localhost:18790"), "canonical form"),
        (entry(DOMAIN, &fp, " 127.0.0.1:18790"), "canonical form"),
        (entry(DOMAIN, &fp, "acme.work.avarok.net"), "canonical form"),
        (entry(DOMAIN, &fp, "127.0.0.1:0"), "not a port"),
        (entry(DOMAIN, &fp, "wss://"), "dial address"),
        (
            entry(DOMAIN, &fp, "wss://user:pw@b.example/"),
            "dial address",
        ),
        (entry(DOMAIN, &fp, ""), "dial address"),
    ];
    for (bad, why) in cases {
        let good = entry("z.example", &fp, "localhost:1");
        let err = AppSessionConfig::new([good, bad.clone()]).unwrap_err();
        assert_eq!((err.entry, err.domain.as_str()), (1, bad.domain.as_str()));
        assert!(err.to_string().contains(why), "{bad:?}: {err}");
    }
}

#[test]
fn a_domain_pinned_twice_is_refused() {
    let twice = [
        entry(DOMAIN, &fp_of(1), DIAL),
        entry(DOMAIN, &fp_of(2), DIAL),
    ];
    let err = AppSessionConfig::new(twice).unwrap_err();
    assert_eq!(err.entry, 1);
    assert!(err.why.contains("more than once"), "{err}");
}

#[test]
fn a_pinned_plan_dials_the_pin_and_registers_under_the_domain() {
    let Discovery::Pinned { server_fp, dial } = config().discovery(DOMAIN) else {
        panic!("{DOMAIN} is pinned");
    };
    let plan = plan::pinned("alice", &dial, plan::dialled(DOMAIN).unwrap(), &server_fp).unwrap();
    assert_eq!(plan.dial, DIAL, "the socket goes to the pin's address");
    assert_eq!(
        plan.domain.as_str(),
        DOMAIN,
        "the challenge names the domain"
    );
    assert_eq!(plan.username, format!("alice@{DOMAIN}"));
    assert_eq!(plan.server_fp, fp_of(1));
    assert_eq!(config().discovery("other.example"), Discovery::WellKnown);
}

#[test]
fn a_changed_pin_is_refused_and_an_unchanged_one_signs_in() {
    let pinned = record(&fp_of(1), false);
    assert_eq!(config().sign_in_again(DOMAIN, DIAL, &pinned), Ok(()));
    let moved = config().sign_in_again(DOMAIN, "127.0.0.1:18791", &pinned);
    assert!(moved
        .unwrap_err()
        .contains("now pins it at 127.0.0.1:18790"));
    let rekeyed = config().sign_in_again(DOMAIN, DIAL, &record(&fp_of(2), false));
    assert!(rekeyed.unwrap_err().contains("nothing was dialled"));
    let discovered = config().sign_in_again(DOMAIN, DIAL, &record(&fp_of(1), true));
    assert!(discovered.unwrap_err().contains("now pins"));
}

#[test]
fn a_pin_removed_after_registration_is_refused() {
    let unpinned = AppSessionConfig::new(Vec::new()).unwrap();
    let why = unpinned
        .sign_in_again(DOMAIN, DIAL, &record(&fp_of(1), false))
        .unwrap_err();
    assert!(why.contains("no longer pins synchronized.work"), "{why}");
    assert!(why.contains(DIAL), "{why}");
    let discovered = record(&fp_of(1), true);
    assert_eq!(unpinned.sign_in_again(DOMAIN, DOMAIN, &discovered), Ok(()));
}
