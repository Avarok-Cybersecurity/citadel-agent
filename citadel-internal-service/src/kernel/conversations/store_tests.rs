use super::*;
use crate::kernel::conversations::kv::{ConversationKv, MemoryKv};
use crate::kernel::conversations::store::{legacy, scoped, MESSAGES_PER_PAGE};
use citadel_internal_service_types::{MessageType, Reaction};

const ME: u64 = 1001;
const PEER: u64 = 2002;

fn message(id: &str, from: u64, at: f64, status: MessageStatus) -> ConversationMessage {
    ConversationMessage {
        id: id.into(),
        content: format!("text of {id}"),
        sender_cid: from,
        recipient_cid: if from == ME { PEER } else { ME },
        timestamp: at,
        index: at,
        status,
        error: None,
        reply_to: None,
        edited_at: None,
        reactions: None,
        mentions: None,
        attachments: None,
        message_type: MessageType::Text,
        document_id: None,
        document_title: None,
        transfer_id: None,
        file_name: None,
        file_size: None,
        file_type: None,
        file_thumbnail: None,
        transfer_mode: None,
        transfer_state: None,
        transfer_progress: None,
        virtual_path: None,
    }
}

fn store(kv: &MemoryKv) -> Store<'_> {
    Store { kv, own: ME }
}

async fn append(s: &Store<'_>, m: &ConversationMessage) -> bool {
    s.append(PEER, m, Some("bob".into()), 1.0)
        .await
        .expect("append")
        .is_some()
}

#[tokio::test]
async fn appending_creates_a_stamped_conversation_and_counts_unread_inbound_only() {
    let kv = MemoryKv::default();
    let s = store(&kv);
    assert!(append(&s, &message("a", ME, 10.0, MessageStatus::Pending)).await);
    assert!(append(&s, &message("b", PEER, 20.0, MessageStatus::Delivered)).await);
    let meta = s.load_metadata(PEER).await.unwrap().unwrap();
    assert_eq!(
        (meta.owner_cid, meta.total_message_count, meta.unread_count),
        (Some(ME), 2.0, 1.0)
    );
    assert_eq!(
        (meta.oldest_message_timestamp, meta.newest_message_timestamp),
        (10.0, 20.0)
    );
    assert!(kv
        .get(&format!("{}_0", scoped(ME, PEER)))
        .await
        .unwrap()
        .is_some());
}

/// A redelivery is stored once, also when its twin sits on the page just closed.
#[tokio::test]
async fn a_redelivered_message_is_stored_once_across_a_page_boundary() {
    let kv = MemoryKv::default();
    let s = store(&kv);
    for i in 0..MESSAGES_PER_PAGE {
        append(
            &s,
            &message(&format!("m{i}"), PEER, i as f64, MessageStatus::Delivered),
        )
        .await;
    }
    assert!(!append(&s, &message("m49", PEER, 49.0, MessageStatus::Delivered)).await);
    assert!(append(&s, &message("m50", PEER, 50.0, MessageStatus::Delivered)).await);
    assert!(
        !append(&s, &message("m49", PEER, 49.0, MessageStatus::Delivered)).await,
        "the twin on page 0 was missed"
    );
    let meta = s.load_metadata(PEER).await.unwrap().unwrap();
    assert_eq!((meta.latest_page, meta.total_message_count), (1, 51.0));
}

/// History written before records were scoped by account is read and adopted;
/// another account's legacy record is not.
#[tokio::test]
async fn legacy_records_are_read_only_when_unstamped_or_ours() {
    let fixture = |name: &str| {
        std::fs::read(format!(
            "{}/tests/fixtures/conversation_store/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    };
    let kv = MemoryKv::default();
    kv.set(
        &format!("{}_metadata", legacy(PEER)),
        fixture("metadata-unattributed.json"),
    )
    .await
    .unwrap();
    kv.set(&format!("{}_0", legacy(PEER)), fixture("page-0.json"))
        .await
        .unwrap();
    let s = store(&kv);
    assert_eq!(
        s.load_page(PEER, 0).await.unwrap().unwrap().messages.len(),
        4
    );
    assert!(
        append(
            &s,
            &message("new", PEER, 1790000009000.0, MessageStatus::Delivered)
        )
        .await
    );
    let scoped_meta = kv
        .get(&format!("{}_metadata", scoped(ME, PEER)))
        .await
        .unwrap();
    assert!(
        scoped_meta.is_some(),
        "the adopted record was not written under this account"
    );
    assert_eq!(
        s.load_metadata(PEER).await.unwrap().unwrap().owner_cid,
        Some(ME)
    );

    let someone_else = Store { kv: &kv, own: 777 };
    kv.set(
        &format!("{}_metadata", legacy(PEER)),
        fixture("metadata.json"),
    )
    .await
    .unwrap(); // owner 1001
    assert!(someone_else.load_metadata(PEER).await.unwrap().is_none());
}

#[path = "store_tests_more.rs"]
mod more;
