use super::*;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/p2p_commands/{name}.cbor",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn plain(message_id: &str) -> Envelope {
    Envelope {
        sender_cid: 1001,
        recipient_cid: 2002,
        message_id: message_id.into(),
        index: 7.0,
        reply_to: None,
        mentions: None,
        attachments: None,
        message_type: MessageType::Text,
        document_id: None,
        document_title: None,
    }
}

/// What the agent sends is byte for byte what the peer's UI would have sent.
#[test]
fn every_builder_writes_the_bytes_the_ui_constructor_writes() {
    let built = [
        (
            "message",
            messaging_layer(
                message_layer("hello", 1790000000123.0),
                &plain("0b5f2b0e-3c8e-4b7a-9f00-2a4c7e1d9a11"),
            ),
        ),
        (
            "edit",
            messaging_layer(
                edit_layer("id-1", "hello, edited", 1790000001000.0),
                &plain("cmd-edit"),
            ),
        ),
        (
            "delete",
            messaging_layer(delete_layer("id-1", 1790000002000.0), &plain("cmd-delete")),
        ),
        (
            "reaction",
            messaging_layer(
                reaction_layer("id-1", "👍", true, 1790000003000.0),
                &plain("cmd-react"),
            ),
        ),
        (
            "ack-delivered",
            ack(AckKind::Delivered, "id-1", 1790000000999.0),
        ),
    ];
    for (name, bytes) in built {
        assert_eq!(
            bytes,
            fixture(name),
            "{name} differs from the UI's encoding"
        );
    }
    let full = Envelope {
        sender_cid: 18446744073709551000,
        recipient_cid: 3,
        message_id: "id-full".into(),
        index: 5000000000.0,
        reply_to: Some("id-parent".into()),
        mentions: Some(vec!["alice".into(), "bob".into()]),
        attachments: Some(vec![Attachment {
            file_id: "f1".into(),
            file_name: "a.txt".into(),
            file_size: 12.0,
            file_type: "text/plain".into(),
            thumbnail: None,
        }]),
        message_type: MessageType::LiveDocument,
        document_id: Some("doc-1".into()),
        document_title: Some("Plan".into()),
    };
    let bytes = messaging_layer(message_layer("héllo 👋 — ünïcode", 1790000000123.5), &full);
    assert_eq!(bytes, fixture("message-full"));
}

#[test]
fn every_kind_the_ui_sends_is_read_for_what_it_is() {
    let Some(Inbound::Message {
        envelope,
        contents,
        timestamp,
    }) = read(&fixture("message-full"))
    else {
        panic!("a message")
    };
    assert_eq!(
        (envelope.sender_cid, envelope.index),
        (18446744073709551000, 5000000000.0)
    );
    assert_eq!(
        envelope.mentions.as_deref(),
        Some(&["alice".to_string(), "bob".to_string()][..])
    );
    assert_eq!(envelope.message_type, MessageType::LiveDocument);
    assert_eq!(
        (contents.as_str(), timestamp),
        ("héllo 👋 — ünïcode", 1790000000123.5)
    );

    assert_eq!(
        read(&fixture("edit")),
        Some(Inbound::Edit {
            target: "id-1".into(),
            contents: "hello, edited".into(),
            edited_at: 1790000001000.0
        })
    );
    assert_eq!(
        read(&fixture("delete")),
        Some(Inbound::Delete {
            target: "id-1".into()
        })
    );
    assert_eq!(
        read(&fixture("reaction")),
        Some(Inbound::Reaction {
            target: "id-1".into(),
            emoji: "👍".into(),
            active: true,
            at: 1790000003000.0
        })
    );
    assert!(
        matches!(read(&fixture("screenshot")), Some(Inbound::Screenshot { taken_at: Some(t), .. }) if t == 1790000004000.0)
    );
    assert_eq!(
        read(&fixture("ack-failed")),
        Some(Inbound::Ack {
            kind: AckKind::Failed,
            message_id: "id-2".into()
        })
    );
    assert_eq!(read(&fixture("typing")), Some(Inbound::Ephemeral));
    assert_eq!(read(&fixture("call-signal")), Some(Inbound::Ephemeral));
    assert!(matches!(
        read(&fixture("message-long")),
        Some(Inbound::Message { .. })
    ));
}

#[test]
fn bytes_that_are_not_a_command_are_not_read_as_one() {
    assert_eq!(read(b"not cbor at all"), None);
    assert_eq!(read(&fixture("reactions")), None, "a list is not a command");
}

/// A forged sender in the payload is still read; the store uses the transport
/// peer instead (see inbound.rs), which this pins the need for.
#[test]
fn the_payload_sender_is_whatever_the_peer_wrote() {
    let forged = messaging_layer(
        message_layer("x", 1.0),
        &Envelope {
            sender_cid: 2002,
            ..plain("f")
        },
    );
    let Some(Inbound::Message { envelope, .. }) = read(&forged) else {
        panic!()
    };
    assert_eq!(envelope.sender_cid, 2002);
}
