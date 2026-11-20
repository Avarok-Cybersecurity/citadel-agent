use super::*;
use citadel_identity::Identity;

const ALICE: u64 = 1;
const BOB: u64 = 2;

fn key(seed: u8) -> Vec<u8> {
    Identity::from_seed(&[seed; 32]).pk().to_vec()
}

#[test]
fn a_peer_is_pinned_once_and_a_later_key_only_waits() {
    let mut book = PinBook::default();
    assert!(book.pin_first(ALICE, BOB, &key(1)));
    assert!(
        !book.pin_first(ALICE, BOB, &key(2)),
        "a pin is never replaced unasked"
    );
    assert!(book.record_change(ALICE, BOB, &key(2)));
    assert!(
        !book.record_change(ALICE, BOB, &key(2)),
        "the same waiting key is told once"
    );
    let pin = book.pin(ALICE, BOB).unwrap();
    assert_eq!(
        (pin.key.clone(), pin.pending.clone()),
        (key(1), Some(key(2)))
    );
    assert!(book.pin(BOB, ALICE).is_none(), "pins are per account");
}

#[test]
fn only_the_key_the_person_was_shown_is_accepted() {
    let mut book = PinBook::default();
    book.pin_first(ALICE, BOB, &key(1));
    assert_eq!(
        book.accept(ALICE, BOB, "x"),
        Err(NotAccepted::NothingPending)
    );
    book.record_change(ALICE, BOB, &key(2));
    let shown = grouped_fingerprint_of(&key(2)).unwrap();
    let other = grouped_fingerprint_of(&key(3)).unwrap();
    assert!(matches!(
        book.accept(ALICE, BOB, &other),
        Err(NotAccepted::OtherKey { .. })
    ));
    assert_eq!(
        book.pin(ALICE, BOB).unwrap().key,
        key(1),
        "a refused accept pins nothing"
    );
    assert_eq!(book.accept(ALICE, BOB, &shown.replace('-', "")), Ok(key(2)));
    let pin = book.pin(ALICE, BOB).unwrap();
    assert_eq!((pin.key.clone(), pin.pending.clone()), (key(2), None));
}

#[test]
fn the_book_survives_its_encoding_and_a_damaged_one_is_refused() {
    let mut book = PinBook::default();
    book.pin_first(ALICE, BOB, &key(1));
    book.record_change(ALICE, BOB, &key(2));
    book.pin_first(BOB, ALICE, &key(3));
    assert_eq!(PinBook::decode(&book.encode()), Ok(book));
    assert!(PinBook::decode(b"[{\"local_cid\":1}]").is_err());
    assert!(PinBook::decode(b"not json").is_err());
}
