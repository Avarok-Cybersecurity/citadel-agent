use super::*;

#[tokio::test]
async fn statuses_only_move_up_the_ladder() {
    let kv = MemoryKv::default();
    let s = store(&kv);
    append(&s, &message("a", ME, 1.0, MessageStatus::Pending)).await;
    assert!(s
        .set_status(PEER, "a", MessageStatus::Delivered, None)
        .await
        .unwrap()
        .is_some());
    assert!(
        s.set_status(PEER, "a", MessageStatus::Sent, None)
            .await
            .unwrap()
            .is_none(),
        "went backwards"
    );
    assert!(s
        .set_status(PEER, "a", MessageStatus::Failed, Some("x".into()))
        .await
        .unwrap()
        .is_none());
    let read = s
        .set_status(PEER, "a", MessageStatus::Read, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.status, MessageStatus::Read);
    assert!(s
        .set_status(PEER, "zzz", MessageStatus::Read, None)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn only_the_sender_may_edit_or_delete() {
    let kv = MemoryKv::default();
    let s = store(&kv);
    append(&s, &message("theirs", PEER, 1.0, MessageStatus::Delivered)).await;
    let edit = || Revision::Edit {
        contents: "changed".into(),
        edited_at: 5.0,
    };
    assert_eq!(
        s.revise(PEER, "theirs", ME, edit(), 9.0).await.unwrap(),
        Revised::NotSender
    );
    let Revised::Applied(m) = s.revise(PEER, "theirs", PEER, edit(), 9.0).await.unwrap() else {
        panic!()
    };
    assert_eq!((m.content.as_str(), m.edited_at), ("changed", Some(5.0)));
    assert!(matches!(
        s.revise(PEER, "theirs", PEER, Revision::Delete, 9.0)
            .await
            .unwrap(),
        Revised::Applied(_)
    ));
    assert!(s.find(PEER, "theirs").await.unwrap().is_none());
    assert_eq!(
        s.load_metadata(PEER)
            .await
            .unwrap()
            .unwrap()
            .total_message_count,
        0.0
    );
}

#[tokio::test]
async fn reactions_fold_on_disk() {
    let kv = MemoryKv::default();
    let s = store(&kv);
    append(&s, &message("a", PEER, 1.0, MessageStatus::Delivered)).await;
    let like = Reaction {
        emoji: "👍".into(),
        reactor_cid: ME,
        at: 10.0,
        active: true,
    };
    assert!(s.react(PEER, "a", &like).await.unwrap().is_some());
    assert!(
        s.react(PEER, "a", &like).await.unwrap().is_none(),
        "the same change applied twice"
    );
    let stored = s.find(PEER, "a").await.unwrap().unwrap();
    assert_eq!(
        stored.1.messages[stored.2].reactions.as_deref(),
        Some(&[like][..])
    );
}

#[tokio::test]
async fn marking_read_covers_the_peers_delivered_messages() {
    let kv = MemoryKv::default();
    let s = store(&kv);
    append(&s, &message("mine", ME, 1.0, MessageStatus::Sent)).await;
    append(&s, &message("x", PEER, 2.0, MessageStatus::Delivered)).await;
    append(&s, &message("y", PEER, 3.0, MessageStatus::Delivered)).await;
    let (read, meta) = s.mark_read(PEER, 9.0).await.unwrap();
    assert_eq!(
        read.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["y", "x"]
    );
    assert_eq!(meta.unwrap().unread_count, 0.0);
    assert!(s.mark_read(PEER, 9.0).await.unwrap().0.is_empty());
}

#[tokio::test]
async fn a_conversation_is_deleted_only_by_its_owner() {
    let kv = MemoryKv::default();
    let s = store(&kv);
    append(&s, &message("a", PEER, 1.0, MessageStatus::Delivered)).await;
    assert!(
        !Store { kv: &kv, own: 777 }
            .delete(PEER, true)
            .await
            .unwrap(),
        "another account deleted it"
    );
    assert!(s.delete(PEER, false).await.unwrap());
    assert!(
        kv.keys().await.unwrap().is_empty(),
        "records were left behind"
    );
}

#[tokio::test]
async fn retention_removes_only_what_is_older_than_the_cutoff() {
    let kv = MemoryKv::default();
    let s = store(&kv);
    for (id, at) in [("old", 1.0), ("older", 0.5), ("new", 100.0)] {
        append(&s, &message(id, PEER, at, MessageStatus::Delivered)).await;
    }
    assert_eq!(s.prune_older_than(PEER, 50.0, 200.0).await.unwrap(), 2);
    let meta = s.load_metadata(PEER).await.unwrap().unwrap();
    assert_eq!(
        (meta.total_message_count, meta.oldest_message_timestamp),
        (1.0, 100.0)
    );
    assert_eq!(s.prune_older_than(PEER, 50.0, 200.0).await.unwrap(), 0);
}

#[tokio::test]
async fn an_account_lists_its_own_and_unstamped_conversations_only() {
    let kv = MemoryKv::default();
    append(
        &store(&kv),
        &message("a", PEER, 1.0, MessageStatus::Delivered),
    )
    .await;
    let other = Store { kv: &kv, own: 777 };
    other
        .append(
            3003,
            &message("b", 3003, 1.0, MessageStatus::Delivered),
            None,
            1.0,
        )
        .await
        .unwrap();
    let mine = store(&kv).list().await.unwrap();
    assert_eq!(mine.iter().map(|m| m.peer_cid).collect::<Vec<_>>(), [PEER]);
}
