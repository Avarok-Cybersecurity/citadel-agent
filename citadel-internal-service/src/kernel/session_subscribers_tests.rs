use super::*;

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

#[test]
fn a_new_session_has_its_opener_as_the_only_and_primary_subscriber() {
    let subs = SessionSubscribers::new(id(1));
    assert_eq!(subs.members(), vec![id(1)]);
    assert_eq!(subs.primary(), Some(id(1)));
    assert!(subs.contains(id(1)));
    assert!(!subs.contains(id(2)));
}

#[test]
fn an_attach_appends_and_leaves_the_primary_alone() {
    let subs = SessionSubscribers::new(id(1));
    assert!(subs.attach(id(2)));
    assert_eq!(subs.members(), vec![id(1), id(2)]);
    assert_eq!(subs.primary(), Some(id(1)));
}

#[test]
fn attaching_twice_does_not_duplicate() {
    let subs = SessionSubscribers::new(id(1));
    assert!(subs.attach(id(2)));
    assert!(!subs.attach(id(2)));
    assert!(!subs.attach(id(1)));
    assert_eq!(subs.members(), vec![id(1), id(2)]);
}

/// The drop rule: a connection that goes detaches only itself.
#[test]
fn a_detach_removes_only_that_connection() {
    let subs = SessionSubscribers::new(id(1));
    subs.attach(id(2));
    subs.attach(id(3));
    let change = subs.detach(id(2));
    assert!(change.removed);
    assert_eq!(change.promoted, None);
    assert_eq!(subs.members(), vec![id(1), id(3)]);
}

/// The primary leaving hands the role to the longest-attached remaining one.
#[test]
fn the_primary_leaving_promotes_the_longest_attached() {
    let subs = SessionSubscribers::new(id(1));
    subs.attach(id(2));
    subs.attach(id(3));
    let change = subs.detach(id(1));
    assert_eq!(change.promoted, Some(id(2)));
    assert_eq!(subs.primary(), Some(id(2)));
}

#[test]
fn detaching_a_stranger_changes_nothing() {
    let subs = SessionSubscribers::new(id(1));
    let change = subs.detach(id(9));
    assert!(!change.removed);
    assert_eq!(subs.members(), vec![id(1)]);
}

/// The last one leaving orphans the session and remembers who held it, which
/// is what lets a reloaded browser reclaim every session its old socket held.
#[test]
fn the_last_drop_orphans_and_remembers_the_holder() {
    let subs = SessionSubscribers::new(id(1));
    let change = subs.detach(id(1));
    assert!(change.removed);
    assert_eq!(change.promoted, None);
    assert!(subs.members().is_empty());
    assert_eq!(subs.primary(), None);
    assert_eq!(subs.last_holder(), Some(id(1)));
}

/// A release is "this window is done with it", not a drop: nobody is
/// remembered, so a later claim of another session does not sweep this one.
#[test]
fn a_release_that_empties_the_set_remembers_nobody() {
    let subs = SessionSubscribers::new(id(1));
    subs.release(id(1));
    assert!(subs.members().is_empty());
    assert_eq!(subs.last_holder(), None);
}

#[test]
fn a_release_by_one_of_several_keeps_the_others() {
    let subs = SessionSubscribers::new(id(1));
    subs.attach(id(2));
    let change = subs.release(id(1));
    assert_eq!(change.promoted, Some(id(2)));
    assert_eq!(subs.members(), vec![id(2)]);
}

/// Today's takeover and orphan claim: the caller becomes the only subscriber.
#[test]
fn a_takeover_displaces_everyone_else() {
    let subs = SessionSubscribers::new(id(1));
    subs.attach(id(2));
    let displaced = subs.take_over(id(3));
    assert_eq!(displaced, vec![id(1), id(2)]);
    assert_eq!(subs.members(), vec![id(3)]);
    assert_eq!(subs.last_holder(), None);
}

/// A connection already attached re-asserting the session must not throw the
/// other windows out: `peer-registration-store/lifecycle.ts` claims its own
/// live session before every PeerRegister.
#[test]
fn a_takeover_by_a_member_keeps_the_set() {
    let subs = SessionSubscribers::new(id(1));
    subs.attach(id(2));
    assert!(subs.take_over(id(2)).is_empty());
    assert_eq!(subs.members(), vec![id(1), id(2)]);
}

#[test]
fn a_takeover_of_an_orphan_clears_the_remembered_holder() {
    let subs = SessionSubscribers::new(id(1));
    subs.detach(id(1));
    assert!(subs.take_over(id(2)).is_empty());
    assert_eq!(subs.members(), vec![id(2)]);
    assert_eq!(subs.last_holder(), None);
}

/// Clones share one set: the route a long-lived task holds must see a change
/// the connection map makes.
#[test]
fn clones_share_one_set() {
    let subs = SessionSubscribers::new(id(1));
    let held_by_a_task = subs.clone();
    subs.attach(id(2));
    assert_eq!(held_by_a_task.members(), vec![id(1), id(2)]);
}

#[test]
fn roles_name_the_primary_first() {
    let subs = SessionSubscribers::new(id(1));
    subs.attach(id(2));
    assert_eq!(
        subs.roles(),
        vec![
            (id(1), SessionRole::Primary),
            (id(2), SessionRole::Secondary)
        ]
    );
}
