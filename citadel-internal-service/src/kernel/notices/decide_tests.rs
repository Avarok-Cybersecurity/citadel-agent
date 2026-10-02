use super::*;

const ME: u64 = 7;
const BOB: u64 = 9;
const SECRET: &str = "the launch codes are 0000";

fn ctx() -> NoticeContext {
    NoticeContext {
        cid: ME,
        account: "alice".into(),
        server_host: Some("acme.work.avarok.net".into()),
        preview: NotificationPreview::SenderOnly,
        muted: false,
        focused: Vec::new(),
    }
}

fn message() -> NoticeSource {
    NoticeSource::Message {
        peer: BOB,
        peer_username: Some("bob".into()),
        text: SECRET.into(),
    }
}

fn call() -> NoticeSource {
    NoticeSource::IncomingCall {
        peer: BOB,
        peer_username: Some("bob".into()),
    }
}

#[test]
fn a_message_names_the_sender_and_hides_the_text_by_default() {
    let notice = decide(&message(), &ctx()).expect("raised");
    assert_eq!(
        (notice.kind, notice.title.as_str(), notice.body.as_str()),
        (NoticeKind::Message, "bob", "New message")
    );
    assert!(!format!("{notice:?}").contains(SECRET));
}

#[test]
fn the_text_appears_only_when_the_account_shows_previews() {
    let shown = NoticeContext {
        preview: NotificationPreview::Text,
        ..ctx()
    };
    assert_eq!(decide(&message(), &shown).expect("raised").body, SECRET);
}

#[test]
fn the_click_target_carries_no_content_whatever_the_preview() {
    for preview in [NotificationPreview::SenderOnly, NotificationPreview::Text] {
        let notice = decide(&message(), &NoticeContext { preview, ..ctx() }).expect("raised");
        assert_eq!(
            notice.target,
            NoticeTarget {
                account: "alice".into(),
                server_host: Some("acme.work.avarok.net".into()),
                open: format!("conversation:{BOB}"),
            }
        );
        let link = format!(
            "{}{:?}{}",
            notice.target.account, notice.target.server_host, notice.target.open
        );
        assert!(!link.contains("launch"), "content in the link: {link}");
    }
}

#[test]
fn a_window_showing_the_conversation_holds_back_its_messages_only() {
    let reading_bob = NoticeContext {
        focused: vec![Some(BOB)],
        ..ctx()
    };
    assert_eq!(decide(&message(), &reading_bob), None);
    let reading_carol = NoticeContext {
        focused: vec![Some(11)],
        ..ctx()
    };
    assert!(decide(&message(), &reading_carol).is_some());
}

#[test]
fn a_focused_account_holds_back_requests_and_calls() {
    let open = NoticeContext {
        focused: vec![None],
        ..ctx()
    };
    assert_eq!(decide(&call(), &open), None);
    let request = NoticeSource::PeerRequest {
        peer: BOB,
        peer_username: None,
    };
    assert_eq!(decide(&request, &open), None);
    assert!(decide(&request, &ctx()).is_some());
}

#[test]
fn a_muted_account_raises_only_calls() {
    let muted = NoticeContext {
        muted: true,
        ..ctx()
    };
    assert_eq!(decide(&message(), &muted), None);
    let offer = NoticeSource::FileOffer {
        peer: BOB,
        peer_username: None,
        file_name: "x.pdf".into(),
    };
    assert_eq!(decide(&offer, &muted), None);
    let ring = decide(&call(), &muted).expect("calls always alert");
    assert_eq!(
        (ring.kind, ring.target.open.as_str()),
        (NoticeKind::IncomingCall, "call:9")
    );
}

#[test]
fn a_file_offer_names_the_file_only_with_previews() {
    let offer = NoticeSource::FileOffer {
        peer: BOB,
        peer_username: Some("bob".into()),
        file_name: "plans.pdf".into(),
    };
    assert_eq!(
        decide(&offer, &ctx()).expect("raised").body,
        "Sent you a file"
    );
    let shown = NoticeContext {
        preview: NotificationPreview::Text,
        ..ctx()
    };
    assert_eq!(
        decide(&offer, &shown).expect("raised").body,
        "Sent you plans.pdf"
    );
}

#[test]
fn a_long_message_is_cut_to_the_preview_length() {
    let long = NoticeSource::Message {
        peer: BOB,
        peer_username: None,
        text: "x".repeat(500),
    };
    let shown = NoticeContext {
        preview: NotificationPreview::Text,
        ..ctx()
    };
    let notice = decide(&long, &shown).expect("raised");
    assert_eq!(
        (notice.body.chars().count(), notice.title.as_str()),
        (PREVIEW_CHARS, "Someone")
    );
}
