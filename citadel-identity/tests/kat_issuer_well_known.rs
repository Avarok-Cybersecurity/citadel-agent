//! `sign-in-kat.json` `issuer.well_known` (identity-api.md §6; sign-in-with-citadel.md §7):
//! work.avarok.net's issuer-only `/.well-known/citadel`, which has no `citadel` member because the
//! control host runs no Citadel node.

mod common;

use citadel_identity::*;
use common::kat::{hex, hex32, load, read_bytes, text, to_hex, variant};
use serde_json::Value;

/// P3. Control: make `citadel` required in `Doc` → this fixture is refused as `Json`.
#[test]
fn the_issuer_only_document_parses_with_no_citadel_endpoint() {
    let k = load("sign-in-kat.json");
    let wk = &k["issuer"]["well_known"];
    assert_eq!(wk["no_citadel"], Value::Bool(true));
    let json = read_bytes(text(wk, "document"));
    let domain = canonical_domain(text(&k["issuer"], "domain")).unwrap();
    let doc = parse_well_known(&json, &domain).unwrap();
    assert_eq!(doc.citadel_wss, None);
    assert_eq!(
        doc.server_key.to_base64url(),
        text(&k["issuer"]["key"], "pk_b64url")
    );

    let expected = [WELL_KNOWN_DOMAIN, &[0], &hex(wk, "signed_cbor_hex")].concat();
    assert_eq!(
        to_hex(&well_known_message(&doc).unwrap()),
        to_hex(&expected)
    );

    let key = ServerKey::from_seed(&hex32(&k["issuer"]["key"], "seed_hex"));
    let ours: Value = serde_json::from_slice(&key.sign_well_known(&doc).unwrap()).unwrap();
    let published: Value = serde_json::from_slice(&json).unwrap();
    assert_eq!(ours, published, "absent stays absent when re-signed");

    for host in wk["refuse_fetched_from"].as_array().unwrap() {
        let from = canonical_domain(host.as_str().unwrap()).unwrap();
        let error = parse_well_known(&json, &from).unwrap_err();
        assert_eq!(variant(&error), "NotThisServer");
    }
    // `"citadel": null` is not absent: refused as a shape error.
    let mut nulled: Value = serde_json::from_slice(&json).unwrap();
    nulled["citadel"] = Value::Null;
    let error = parse_well_known(&serde_json::to_vec(&nulled).unwrap(), &domain).unwrap_err();
    assert_eq!(variant(&error), "Json");
}
