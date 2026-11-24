//! Every `fixtures/reduce` case: folded as its volume folds; a Merge volume's in every order.

mod common;

use common::fold::{fold, load};
use common::show::assert_matches;

fn run(text: &str) {
    let f = load(text);
    let tree = fold(&f, &f.ops);
    assert_matches(&f.name, &tree, &f.json["expect"]);
    if f.volume.mode() == citadel_vfs::VolumeMode::Merge {
        for order in common::orders(f.ops.len()) {
            let ops: Vec<_> = order.iter().map(|i| f.ops[*i].clone()).collect();
            assert_eq!(fold(&f, &ops), tree, "{}: order {order:?}", f.name);
        }
    }
}

both! {
    fn r01() { run(include_str!("contracts/rel-1124/reduce/01-create-place.json")) }
    fn r02() { run(include_str!("contracts/rel-1124/reduce/02-order-is-the-stamp-not-arrival.json")) }
    fn r03() { run(include_str!("contracts/rel-1124/reduce/03-lww-tie-break.json")) }
    fn r04() { run(include_str!("contracts/rel-1124/reduce/04-cycle-is-skipped.json")) }
    fn r05() { run(include_str!("contracts/rel-1124/reduce/05-no-parent.json")) }
    fn r06() { run(include_str!("contracts/rel-1124/reduce/06-create-merges.json")) }
    fn r07() { run(include_str!("contracts/rel-1124/reduce/07-names-shown.json")) }
    fn r08a() { run(include_str!("contracts/rel-1124/reduce/08a-edits-to-a-removed-entry.json")) }
    fn r08b() { run(include_str!("contracts/rel-1124/reduce/08b-restore-brings-the-edit-back.json")) }
    fn r09() { run(include_str!("contracts/rel-1124/reduce/09-remove-and-restore-are-one-register.json")) }
    fn r10() { run(include_str!("contracts/rel-1124/reduce/10-purge-is-terminal.json")) }
    fn r11a() { run(include_str!("contracts/rel-1124/reduce/11a-purge-of-a-folder-with-contents.json")) }
    fn r11b() { run(include_str!("contracts/rel-1124/reduce/11b-purge-deepest-first.json")) }
    fn r12() { run(include_str!("contracts/rel-1124/reduce/12-authorize-at-a-position.json")) }
    fn r13() { run(include_str!("contracts/rel-1124/reduce/13-a-host-waits-before-it-purges.json")) }
    fn r14() { run(include_str!("contracts/rel-1124/reduce/14-what-a-server-may-write.json")) }
    fn r15() { run(include_str!("contracts/rel-1124/reduce/15-one-op-id-two-ops.json")) }
    fn r16() { run(include_str!("contracts/rel-1124/reduce/16-depth-limit.json")) }
    fn r17() { run(include_str!("contracts/rel-1124/reduce/17-a-backdated-stamp-has-no-effect.json")) }
    fn r18() { run(include_str!("contracts/rel-1124/reduce/18-the-group-owner-administers.json")) }
}
