use super::*;
use citadel_internal_service_types::{MessageStatus, MessageType, TransferState};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/conversation_store/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("fixture")
}

/// History the web UI wrote reads into the canonical types, every field.
#[test]
fn a_page_the_ui_wrote_reads_back_whole() {
    let page = decode_page(&fixture("page-0.json")).expect("decodes");
    assert_eq!(
        (page.peer_cid, page.page_number, page.messages.len()),
        (2002, 0, 4)
    );
    let [m1, m2, m3, m4] = &page.messages[..] else {
        unreachable!()
    };
    assert_eq!(
        (m1.sender_cid, m1.status, m1.message_type),
        (1001, MessageStatus::Read, MessageType::Text)
    );
    assert_eq!(m2.timestamp, 1790000001000.25);
    assert_eq!(m2.reply_to.as_deref(), Some("m1"));
    assert_eq!(m2.edited_at, Some(1790000002000.0));
    let reactions = m2.reactions.as_ref().expect("reactions");
    assert_eq!(
        (reactions[0].reactor_cid, reactions[1].active),
        (1001, false)
    );
    assert_eq!(m3.transfer_state, Some(TransferState::Staged));
    assert_eq!(m3.file_size, Some(123456.0));
    assert_eq!(m3.attachments.as_ref().map(Vec::len), Some(1));
    assert_eq!(
        (m4.status, m4.error.as_deref()),
        (MessageStatus::Failed, Some("Could not be saved"))
    );
    assert_eq!(m4.document_title.as_deref(), Some("Doc"));
}

/// What the agent writes is what the UI's reader expects: re-encoding the UI's
/// page gives JSON equal to the UI's, value for value.
#[test]
fn the_agent_writes_the_same_json_the_ui_wrote() {
    let original: serde_json::Value = serde_json::from_slice(&fixture("page-0.json")).unwrap();
    let rewritten = encode_page(&decode_page(&fixture("page-0.json")).unwrap()).unwrap();
    let rewritten: serde_json::Value = serde_json::from_slice(&rewritten).unwrap();
    assert_eq!(rewritten, original);

    let original: serde_json::Value = serde_json::from_slice(&fixture("metadata.json")).unwrap();
    let rewritten = encode_metadata(&decode_metadata(&fixture("metadata.json")).unwrap()).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&rewritten).unwrap(),
        original
    );
}

#[test]
fn metadata_with_and_without_an_owner_stamp() {
    let owned = decode_metadata(&fixture("metadata.json")).unwrap();
    assert_eq!((owned.peer_cid, owned.owner_cid), (2002, Some(1001)));
    assert_eq!(owned.peer_username.as_deref(), Some("bob"));
    let unattributed = decode_metadata(&fixture("metadata-unattributed.json")).unwrap();
    assert_eq!(unattributed.owner_cid, None);
}

#[test]
fn a_corrupt_record_is_an_error_not_an_empty_one() {
    assert!(decode_page(b"{not json").is_err());
    assert!(decode_metadata(br#"{"peerCid":"not a number"}"#).is_err());
}
