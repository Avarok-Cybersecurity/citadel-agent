//! `Notify` grew a kind and the change it is about. An app built before either still sends the
//! same bytes and is still understood; one that says nothing more sends nothing more.

use citadel_app_sdk_types::calls::{Notify, NotifyAbout, NotifyKind};
use citadel_app_sdk_types::{decode, encode};

/// CBOR `{"title": "t", "body": "b"}`: what every app sent before `kind` and `about`.
const OLD: &[u8] = b"\xa2\x65title\x61t\x64body\x61b";

fn plain() -> Notify {
    Notify {
        title: "t".into(),
        body: "b".into(),
        kind: None,
        about: None,
    }
}

#[test]
fn an_older_apps_notice_is_read_as_one_that_says_nothing_more() {
    assert_eq!(decode::<Notify>(OLD).unwrap(), plain());
}

#[test]
fn a_notice_that_says_nothing_more_sends_the_old_bytes() {
    assert_eq!(encode(&plain()), OLD);
}

#[test]
fn a_kind_and_the_change_it_is_about_travel_whole() {
    let n = Notify {
        kind: Some(NotifyKind::Invitation),
        about: Some(NotifyAbout {
            node_id: "n".into(),
            author: "alice".into(),
            key: "cal/ev/1".into(),
        }),
        ..plain()
    };
    assert_eq!(decode::<Notify>(&encode(&n)).unwrap(), n);
}
