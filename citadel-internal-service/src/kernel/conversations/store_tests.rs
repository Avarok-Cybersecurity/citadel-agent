use super::*;
use crate::kernel::conversations::kv::{ConversationKv, MemoryKv};
use crate::kernel::conversations::store::{scoped, MESSAGES_PER_PAGE, PAGINATED_PREFIX};
use crate::kernel::conversations::stored;
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
        (ME, 2.0, 1.0)
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

/// A record under the peer alone was written before keys were scoped by
/// account: it is not read, listed or adopted, and a write starts a new one.
#[tokio::test]
async fn an_unscoped_record_is_never_read() {
    let kv = MemoryKv::default();
    let unscoped = format!("{PAGINATED_PREFIX}{PEER}");
    kv.set(
        &format!("{unscoped}_metadata"),
        stored::encode_metadata(&stored::tests::metadata(PEER, ME)).unwrap(),
    )
    .await
    .unwrap();
    kv.set(
        &format!("{unscoped}_0"),
        stored::encode_page(&stored::tests::page(PEER, ME)).unwrap(),
    )
    .await
    .unwrap();
    let s = store(&kv);
    assert!(s.load_metadata(PEER).await.unwrap().is_none());
    assert!(s.load_page(PEER, 0).await.unwrap().is_none());
    assert!(s.list().await.unwrap().is_empty());
    assert!(
        append(
            &s,
            &message("new", PEER, 1790000009000.0, MessageStatus::Delivered)
        )
        .await
    );
    let meta = s.load_metadata(PEER).await.unwrap().unwrap();
    assert_eq!((meta.owner_cid, meta.total_message_count), (ME, 1.0));
}

#[path = "store_tests_more.rs"]
mod more;
