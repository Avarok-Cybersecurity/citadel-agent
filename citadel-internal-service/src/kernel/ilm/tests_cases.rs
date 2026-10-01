//! The cases, over the rig in tests.rs.

use super::*;

#[tokio::test]
async fn a_message_crosses_between_two_agent_hosted_ilms_once_and_in_order() {
    let (mut alice, mut bob) = pair();
    let (a, b) = (host(&alice).await, host(&bob).await);

    for body in [b"one".as_slice(), b"two", b"three"] {
        send(&a, BOB, body).await;
    }
    for expected in [b"one".as_slice(), b"two", b"three"] {
        let got = next(&mut bob.delivered).await;
        assert_eq!(got.message, expected, "out of order or altered");
        assert_eq!((got.cid, got.peer_cid), (BOB, ALICE));
    }
    send(&b, ALICE, b"reply").await;
    assert_eq!(next(&mut alice.delivered).await.message, b"reply");
    assert!(
        tokio::time::timeout(Duration::from_millis(1500), bob.delivered.recv())
            .await
            .is_err(),
        "a message was delivered twice"
    );
}

/// No window attached: delivery is refused, the message is kept (and not
/// acknowledged), and it arrives the moment someone can take it.
#[tokio::test]
async fn a_message_nobody_can_take_yet_is_kept_not_lost() {
    let (alice, mut bob) = pair();
    let a = host(&alice).await;
    host(&bob).await;
    bob.io.accepts.store(false, Ordering::SeqCst);

    send(&a, BOB, b"while away").await;
    assert!(
        tokio::time::timeout(Duration::from_millis(1500), bob.delivered.recv())
            .await
            .is_err(),
        "delivered although nobody could take it"
    );
    bob.io.accepts.store(true, Ordering::SeqCst);
    assert_eq!(next(&mut bob.delivered).await.message, b"while away");
}

/// What an earlier ILM for the account left queued under the account's keys is
/// picked up and sent by the next one. A browser's ILM writes those same keys
/// through the same backend (the connector's `through_any_channel` tests), so
/// this is the hand-over from a browser to the agent: nothing is lost.
#[tokio::test]
async fn a_new_host_sends_what_an_earlier_one_left_queued() {
    let (alice, mut bob) = pair();
    host(&bob).await;
    // Bob is unreachable while the first ILM runs: its frames go nowhere.
    let reachable_bob = std::mem::take(&mut *alice.io.peer_registry.lock());
    {
        let first = host(&alice).await;
        send(&first, BOB, b"queued, never sent").await;
    }
    alice.registry.stop(ALICE);
    let pending =
        CitadelWorkspaceBackend::with_channel(ALICE, channel::AgentChannel::new(alice.io.clone()))
            .get_pending_outbound()
            .await
            .expect("the account's store is readable");
    assert_eq!(pending.len(), 1, "the message was not left queued");
    assert!(
        bob.delivered.try_recv().is_err(),
        "it reached Bob before the hand-over"
    );

    *alice.io.peer_registry.lock() = reachable_bob;
    host(&alice).await;
    assert_eq!(
        next(&mut bob.delivered).await.message,
        b"queued, never sent"
    );
}

#[tokio::test]
async fn one_ilm_per_account() {
    let (alice, _bob) = pair();
    let first = host(&alice).await;
    let again = host(&alice).await;
    assert!(
        Arc::ptr_eq(&first, &again),
        "a second ILM was started for one account"
    );
    alice.registry.stop(ALICE);
    assert!(!alice.registry.is_hosted(ALICE));
    let fresh = host(&alice).await;
    assert!(!Arc::ptr_eq(&first, &fresh));
}

/// Raw traffic (Yjs, the plain messaging service) and unhosted accounts are not
/// the ILM's: the notification comes back untouched for the raw path.
#[tokio::test]
async fn what_is_not_an_ilm_frame_is_handed_back() {
    let (alice, _bob) = pair();
    let raw = MessageNotification {
        message: b"yjs update".to_vec(),
        cid: ALICE,
        peer_cid: BOB,
        request_id: None,
    };
    let handed_back = |result: Result<(), MessageNotification>| match result {
        Err(n) => n.message == raw.message && n.cid == raw.cid,
        Ok(()) => false,
    };
    assert!(handed_back(alice.registry.feed(raw.clone())), "unhosted");
    host(&alice).await;
    assert!(handed_back(alice.registry.feed(raw.clone())), "not a frame");
}
