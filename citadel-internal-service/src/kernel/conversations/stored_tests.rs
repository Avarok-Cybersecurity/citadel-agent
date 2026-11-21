use super::*;
use citadel_internal_service_types::{
    Attachment, ConversationMessage, MessageStatus, MessageType, PageTimestamps, Reaction,
    TransferState,
};

pub(crate) fn metadata(peer: u64, owner: u64) -> ConversationMetadata {
    ConversationMetadata {
        peer_cid: peer,
        owner_cid: owner,
        peer_username: Some("bob".into()),
        total_message_count: 1.0,
        oldest_message_timestamp: 1790000000000.0,
        newest_message_timestamp: 1790000000000.0,
        latest_page: 0,
        messages_per_page: 50,
        unread_count: 1.0,
        last_message_index: 1.0,
        last_updated: 1790000006000.0,
    }
}

pub(crate) fn page(peer: u64, owner: u64) -> ConversationPage {
    let message = ConversationMessage {
        id: "m1".into(),
        content: "report.pdf".into(),
        sender_cid: owner,
        recipient_cid: peer,
        timestamp: 1790000001000.25,
        index: 1.0,
        status: MessageStatus::Sent,
        error: Some("Could not be saved".into()),
        reply_to: Some("m0".into()),
        edited_at: Some(1790000002000.0),
        reactions: Some(vec![Reaction {
            emoji: "👍".into(),
            reactor_cid: u64::MAX,
            at: 1790000003000.0,
            active: false,
        }]),
        mentions: Some(vec!["alice".into()]),
        attachments: Some(vec![Attachment {
            file_id: "f1".into(),
            file_name: "report.pdf".into(),
            file_size: 123456.0,
            file_type: "application/pdf".into(),
            thumbnail: None,
        }]),
        message_type: MessageType::Text,
        document_id: None,
        document_title: Some("Doc".into()),
        transfer_id: Some("t1".into()),
        file_name: Some("report.pdf".into()),
        file_size: Some(123456.0),
        file_type: None,
        file_thumbnail: None,
        transfer_mode: None,
        transfer_state: Some(TransferState::Staged),
        transfer_progress: Some(0.5),
        virtual_path: None,
    };
    ConversationPage {
        peer_cid: peer,
        page_number: 0,
        messages: vec![message],
        page_timestamps: PageTimestamps {
            min_timestamp: 1790000001000.25,
            max_timestamp: 1790000001000.25,
        },
    }
}

#[test]
fn a_page_and_its_metadata_read_back_whole() {
    let written = page(2002, 1001);
    assert_eq!(
        decode_page(&encode_page(&written).unwrap()).unwrap(),
        written
    );
    let written = metadata(2002, 1001);
    assert_eq!(
        decode_metadata(&encode_metadata(&written).unwrap()).unwrap(),
        written
    );
}

#[test]
fn a_record_without_its_owner_is_refused() {
    let mut value = serde_json::to_value(metadata(2002, 1001)).unwrap();
    value.as_object_mut().unwrap().remove("ownerCid");
    assert!(decode_metadata(&serde_json::to_vec(&value).unwrap()).is_err());
}

/// The layout the web UI once wrote (CIDs as decimal strings, reactions as cbor-x
/// bytes) is not this format, and is refused rather than read.
#[test]
fn a_record_in_the_earlier_ui_layout_is_refused() {
    let mut value = serde_json::to_value(metadata(2002, 1001)).unwrap();
    value["peerCid"] = serde_json::Value::from("2002");
    assert!(decode_metadata(&serde_json::to_vec(&value).unwrap()).is_err());
    let mut value = serde_json::to_value(page(2002, 1001)).unwrap();
    value["messages"][0]["reactions"] = serde_json::json!([0x81, 0xa0]);
    assert!(decode_page(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn a_corrupt_record_is_an_error_not_an_empty_one() {
    assert!(decode_page(b"{not json").is_err());
    assert!(decode_metadata(br#"{"peerCid":"not a number"}"#).is_err());
}
