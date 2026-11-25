//! The copied fixtures are byte-identical to `PROVENANCE`: a changed or dropped file fails here.

mod common;

use sha2::{Digest, Sha256};

const FILES: &[(&str, &[u8])] = &[
    (
        "authorize/cases.json",
        include_bytes!("contracts/rel-1124/authorize/cases.json"),
    ),
    (
        "codec/01-create-dir.json",
        include_bytes!("contracts/rel-1124/codec/01-create-dir.json"),
    ),
    (
        "codec/02-create-file.json",
        include_bytes!("contracts/rel-1124/codec/02-create-file.json"),
    ),
    (
        "codec/03-set-pointee-chained.json",
        include_bytes!("contracts/rel-1124/codec/03-set-pointee-chained.json"),
    ),
    (
        "codec/04-remove.json",
        include_bytes!("contracts/rel-1124/codec/04-remove.json"),
    ),
    (
        "codec/05-server-create.json",
        include_bytes!("contracts/rel-1124/codec/05-server-create.json"),
    ),
    (
        "codec/06-admit-member.json",
        include_bytes!("contracts/rel-1124/codec/06-admit-member.json"),
    ),
    (
        "codec/07-dismiss.json",
        include_bytes!("contracts/rel-1124/codec/07-dismiss.json"),
    ),
    (
        "codec/08-guest-set-pointee.json",
        include_bytes!("contracts/rel-1124/codec/08-guest-set-pointee.json"),
    ),
    (
        "codec/inode-kat.json",
        include_bytes!("contracts/rel-1124/codec/inode-kat.json"),
    ),
    (
        "codec/invalid-nonshortest-lamport.json",
        include_bytes!("contracts/rel-1124/codec/invalid-nonshortest-lamport.json"),
    ),
    (
        "codec/invalid-null-prev.json",
        include_bytes!("contracts/rel-1124/codec/invalid-null-prev.json"),
    ),
    (
        "codec/invalid-unsorted-header.json",
        include_bytes!("contracts/rel-1124/codec/invalid-unsorted-header.json"),
    ),
    (
        "codec/sign-kat.json",
        include_bytes!("contracts/rel-1124/codec/sign-kat.json"),
    ),
    (
        "names/cases.json",
        include_bytes!("contracts/rel-1124/names/cases.json"),
    ),
    (
        "reduce/01-create-place.json",
        include_bytes!("contracts/rel-1124/reduce/01-create-place.json"),
    ),
    (
        "reduce/02-order-is-the-stamp-not-arrival.json",
        include_bytes!("contracts/rel-1124/reduce/02-order-is-the-stamp-not-arrival.json"),
    ),
    (
        "reduce/03-lww-tie-break.json",
        include_bytes!("contracts/rel-1124/reduce/03-lww-tie-break.json"),
    ),
    (
        "reduce/04-cycle-is-skipped.json",
        include_bytes!("contracts/rel-1124/reduce/04-cycle-is-skipped.json"),
    ),
    (
        "reduce/05-no-parent.json",
        include_bytes!("contracts/rel-1124/reduce/05-no-parent.json"),
    ),
    (
        "reduce/06-create-merges.json",
        include_bytes!("contracts/rel-1124/reduce/06-create-merges.json"),
    ),
    (
        "reduce/07-names-shown.json",
        include_bytes!("contracts/rel-1124/reduce/07-names-shown.json"),
    ),
    (
        "reduce/08a-edits-to-a-removed-entry.json",
        include_bytes!("contracts/rel-1124/reduce/08a-edits-to-a-removed-entry.json"),
    ),
    (
        "reduce/08b-restore-brings-the-edit-back.json",
        include_bytes!("contracts/rel-1124/reduce/08b-restore-brings-the-edit-back.json"),
    ),
    (
        "reduce/09-remove-and-restore-are-one-register.json",
        include_bytes!("contracts/rel-1124/reduce/09-remove-and-restore-are-one-register.json"),
    ),
    (
        "reduce/10-purge-is-terminal.json",
        include_bytes!("contracts/rel-1124/reduce/10-purge-is-terminal.json"),
    ),
    (
        "reduce/11a-purge-of-a-folder-with-contents.json",
        include_bytes!("contracts/rel-1124/reduce/11a-purge-of-a-folder-with-contents.json"),
    ),
    (
        "reduce/11b-purge-deepest-first.json",
        include_bytes!("contracts/rel-1124/reduce/11b-purge-deepest-first.json"),
    ),
    (
        "reduce/12-authorize-at-a-position.json",
        include_bytes!("contracts/rel-1124/reduce/12-authorize-at-a-position.json"),
    ),
    (
        "reduce/13-a-host-waits-before-it-purges.json",
        include_bytes!("contracts/rel-1124/reduce/13-a-host-waits-before-it-purges.json"),
    ),
    (
        "reduce/14-what-a-server-may-write.json",
        include_bytes!("contracts/rel-1124/reduce/14-what-a-server-may-write.json"),
    ),
    (
        "reduce/15-one-op-id-two-ops.json",
        include_bytes!("contracts/rel-1124/reduce/15-one-op-id-two-ops.json"),
    ),
    (
        "reduce/16-depth-limit.json",
        include_bytes!("contracts/rel-1124/reduce/16-depth-limit.json"),
    ),
    (
        "reduce/17-a-backdated-stamp-has-no-effect.json",
        include_bytes!("contracts/rel-1124/reduce/17-a-backdated-stamp-has-no-effect.json"),
    ),
    (
        "reduce/18-the-group-owner-administers.json",
        include_bytes!("contracts/rel-1124/reduce/18-the-group-owner-administers.json"),
    ),
    (
        "routing/shapes.json",
        include_bytes!("contracts/rel-1124/routing/shapes.json"),
    ),
];

both! {
    fn fixtures_match_provenance() {
        let manifest = include_str!("contracts/rel-1124/PROVENANCE");
        let listed: Vec<(&str, &str)> = manifest
            .lines()
            .filter_map(|l| l.split_once("  "))
            .filter(|(h, _)| h.len() == 64)
            .map(|(h, f)| (h, f.trim_start_matches("./")))
            .collect();
        assert_eq!(listed.len(), FILES.len(), "every listed file is checked");
        assert!(manifest.contains("@ 42c90a294e403470a6ec5780ae72e5bea85d85c0"), "source commit");
        for ((hash, name), (file, bytes)) in listed.iter().zip(FILES) {
            assert_eq!(name, file, "manifest order");
            let got: String = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(&got, hash, "{file} drifted from PROVENANCE");
        }
    }
}
