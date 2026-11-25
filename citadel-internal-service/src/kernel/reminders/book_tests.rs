use super::*;

const CAL: &str = "net.avarok.calendar";

pub(crate) fn reminder(id: &str, at: u64) -> AppNotice {
    AppNotice {
        id: id.to_string(),
        app_id: CAL.to_string(),
        node_id: "workspace-root".to_string(),
        title: "Standup".to_string(),
        body: "Starts at 10:00".to_string(),
        sender_only: None,
        fire_at_ms: Some(at),
        start_ms: Some(at + 600_000),
    }
}

fn ids(due: &[(AppNotice, bool)]) -> Vec<(&str, bool)> {
    due.iter().map(|(n, late)| (n.id.as_str(), *late)).collect()
}

#[test]
fn what_is_due_fires_once_in_time_order() {
    let mut book = Book::empty()
        .sync(
            CAL,
            vec![
                reminder("b", 2_000),
                reminder("a", 1_000),
                reminder("c", 9_000),
            ],
        )
        .unwrap();
    assert_eq!(book.next_at(), Some(1_000));
    assert_eq!(ids(&book.take_due(2_000)), vec![("a", false), ("b", false)]);
    assert!(book.take_due(2_000).is_empty());
    assert_eq!(book.next_at(), Some(9_000));
}

#[test]
fn a_resync_never_takes_back_what_fired() {
    let mut book = Book::empty().sync(CAL, vec![reminder("a", 1_000)]).unwrap();
    book.take_due(1_000);
    let mut again = book.sync(CAL, vec![reminder("a", 1_000)]).unwrap();
    assert!(again.take_due(5_000).is_empty());
}

#[test]
fn a_fired_id_the_page_dropped_is_forgotten() {
    let mut book = Book::empty().sync(CAL, vec![reminder("a", 1_000)]).unwrap();
    book.take_due(1_000);
    let book = book.sync(CAL, Vec::new()).unwrap();
    assert!(book.fired.is_empty());
}

#[test]
fn a_sync_cancels_what_it_leaves_out_and_spares_other_apps() {
    let mut other = reminder("o", 1_000);
    other.app_id = "net.avarok.other".to_string();
    let book = Book::empty().sync("net.avarok.other", vec![other]).unwrap();
    let mut book = book.sync(CAL, vec![reminder("a", 1_000)]).unwrap();
    let mut book2 = book.sync(CAL, Vec::new()).unwrap();
    assert_eq!(ids(&book2.take_due(1_000)), vec![("o", false)]);
    assert_eq!(book.take_due(1_000).len(), 2);
}

#[test]
fn past_the_late_mark_a_reminder_is_late() {
    let mut book = Book::empty().sync(CAL, vec![reminder("a", 1_000)]).unwrap();
    assert_eq!(
        ids(&book.take_due(1_000 + LATE_AFTER_MS + 1)),
        vec![("a", true)]
    );
}

#[test]
fn past_the_cap_nothing_is_stored() {
    let many: Vec<AppNotice> = (0..=CAP as u64)
        .map(|i| reminder(&format!("r{i}"), i))
        .collect();
    let refused = Book::empty().sync(CAL, many);
    assert_eq!(
        refused,
        Err(AppNoticeRefusal::CapExceeded { cap: CAP as u32 })
    );
    let at_cap: Vec<AppNotice> = (0..CAP as u64)
        .map(|i| reminder(&format!("r{i}"), i))
        .collect();
    assert!(Book::empty().sync(CAL, at_cap).is_ok());
}

#[test]
fn a_target_that_is_not_a_node_id_is_refused() {
    for node in [
        "https://evil.example/",
        "a/b",
        "a?b",
        "a#b",
        "",
        &"n".repeat(129),
    ] {
        let mut bad = reminder("a", 1);
        bad.node_id = node.to_string();
        assert_eq!(
            Book::empty().sync(CAL, vec![bad]),
            Err(AppNoticeRefusal::BadTarget),
            "{node}"
        );
    }
}

#[test]
fn a_reminder_without_a_time_or_for_another_app_is_refused() {
    let mut untimed = reminder("a", 1);
    untimed.fire_at_ms = None;
    assert_eq!(
        Book::empty().sync(CAL, vec![untimed]),
        Err(AppNoticeRefusal::BadTarget)
    );
    let mut other = reminder("a", 1);
    other.app_id = "net.avarok.other".to_string();
    assert_eq!(
        Book::empty().sync(CAL, vec![other]),
        Err(AppNoticeRefusal::BadTarget)
    );
}

#[test]
fn held_notices_are_bounded_and_taken_once() {
    let mut book = Book::empty();
    for i in 0..=CAP as u64 {
        book.hold(reminder(&format!("h{i}"), i));
    }
    let held = book.take_held();
    assert_eq!(held.len(), CAP);
    assert_eq!(held[0].id, "h1");
    assert!(book.take_held().is_empty());
}

#[test]
fn the_book_survives_its_encoding() {
    let mut book = Book::empty()
        .sync(CAL, vec![reminder("a", 1), reminder("b", 5)])
        .unwrap();
    book.take_due(1);
    book.hold(reminder("h", 0));
    assert_eq!(Book::decode(&book.encode().unwrap()).unwrap(), book);
}

#[test]
fn a_sender_line_must_be_one_short_line() {
    let with = |line: &str| AppNotice {
        sender_only: Some(line.to_string()),
        ..reminder("a", 1)
    };
    assert!(acceptable(&with("Calendar: invitation from bob")));
    assert!(!acceptable(&with("")));
    assert!(!acceptable(&with("Calendar\nthe launch codes")));
    assert!(!acceptable(&with(&"x".repeat(TITLE_MAX + 1))));
}
