//! `local` storage over an in-memory store: limits, paging and isolation.

use super::*;
use crate::kernel::conversations::kv::MemoryKv;
use citadel_internal_service_types::APP_KV_MAX_KEY_BYTES;

fn ns(user: &str) -> Namespace {
    Namespace::new("com.acme.kanban", "workspace", user).expect("valid")
}

async fn set(
    kv: &AppKv,
    store: &MemoryKv,
    ns: &Namespace,
    key: &str,
    value: &[u8],
) -> Result<AppKvResult, AppKvRefusal> {
    let op = AppKvOp::Set {
        key: key.into(),
        value: value.to_vec(),
    };
    kv.run(store, ns, op).await
}

async fn get(kv: &AppKv, store: &MemoryKv, ns: &Namespace, key: &str) -> Option<Vec<u8>> {
    match kv.run(store, ns, AppKvOp::Get { key: key.into() }).await {
        Ok(AppKvResult::Value(value)) => value,
        other => panic!("get {key}: {other:?}"),
    }
}

#[tokio::test]
async fn set_get_delete_round_trip_and_stay_in_their_namespace() {
    let (kv, store) = (AppKv::default(), MemoryKv::default());
    let alice = ns("alice");
    set(&kv, &store, &alice, "board", b"v1").await.expect("set");
    assert_eq!(
        get(&kv, &store, &alice, "board").await,
        Some(b"v1".to_vec())
    );
    // Bob, with the same app and scope, does not see it.
    assert_eq!(get(&kv, &store, &ns("bob"), "board").await, None);
    // Every record written belongs to Alice's namespace.
    assert!(store.0.lock().keys().all(|k| alice.owns(k)));
    kv.run(
        &store,
        &alice,
        AppKvOp::Delete {
            key: "board".into(),
        },
    )
    .await
    .expect("delete");
    assert_eq!(get(&kv, &store, &alice, "board").await, None);
}

#[tokio::test]
async fn a_crafted_key_reads_and_writes_only_its_own_entry() {
    let (kv, store) = (AppKv::default(), MemoryKv::default());
    let (alice, bob) = (ns("alice"), ns("bob"));
    set(&kv, &store, &bob, "secret", b"bob's")
        .await
        .expect("set");
    let bobs_record = bob.entry("secret").unwrap();
    for crafted in ["../bob/secret", &bobs_record, "i", ":i", "..", "k:secret"] {
        assert_eq!(get(&kv, &store, &alice, crafted).await, None, "{crafted}");
        set(&kv, &store, &alice, crafted, b"overwrite")
            .await
            .expect(crafted);
    }
    assert_eq!(
        get(&kv, &store, &bob, "secret").await,
        Some(b"bob's".to_vec())
    );
    match kv
        .run(
            &store,
            &bob,
            AppKvOp::List {
                prefix: None,
                after: None,
            },
        )
        .await
    {
        Ok(AppKvResult::Keys { keys, next }) => {
            assert_eq!(keys, vec!["secret".to_string()]);
            assert_eq!(next, None);
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn the_key_value_and_total_limits_hold() {
    let (kv, store) = (AppKv::default(), MemoryKv::default());
    let alice = ns("alice");
    let too_long = "k".repeat(APP_KV_MAX_KEY_BYTES + 1);
    assert_eq!(
        set(&kv, &store, &alice, &too_long, b"").await,
        Err(AppKvRefusal::Key)
    );
    let big = vec![0u8; APP_KV_MAX_VALUE_BYTES + 1];
    assert_eq!(
        set(&kv, &store, &alice, "big", &big).await,
        Err(AppKvRefusal::Value)
    );
    // Fill to just under the total with maximum values.
    let value = vec![1u8; APP_KV_MAX_VALUE_BYTES];
    let mut stored = 0;
    loop {
        let key = format!("v{stored:02}");
        match set(&kv, &store, &alice, &key, &value).await {
            Ok(_) => stored += 1,
            Err(AppKvRefusal::Quota) => break,
            Err(other) => panic!("{other:?}"),
        }
    }
    assert_eq!(
        stored, 15,
        "4 MiB holds fifteen 256 KiB values with their overhead"
    );
    // Replacing an entry with a smaller value is allowed at the limit, and frees room.
    set(&kv, &store, &alice, "v00", b"small")
        .await
        .expect("shrink");
    set(&kv, &store, &alice, "another", &value[..1024])
        .await
        .expect("room again");
    // Another account's namespace has its own total.
    set(&kv, &store, &ns("bob"), "v00", &value)
        .await
        .expect("bob's own quota");
}

#[tokio::test]
async fn listing_pages_by_one_hundred_in_key_order() {
    let (kv, store) = (AppKv::default(), MemoryKv::default());
    let alice = ns("alice");
    for n in 0..250 {
        set(&kv, &store, &alice, &format!("item/{n:03}"), b"x")
            .await
            .expect("set");
    }
    set(&kv, &store, &alice, "other", b"x").await.expect("set");
    let mut after = None;
    let mut seen = Vec::new();
    loop {
        let op = AppKvOp::List {
            prefix: Some("item/".into()),
            after: after.clone(),
        };
        let Ok(AppKvResult::Keys { keys, next }) = kv.run(&store, &alice, op).await else {
            panic!("list")
        };
        assert!(keys.len() <= citadel_internal_service_types::APP_KV_LIST_PAGE);
        seen.extend(keys);
        match next {
            Some(cursor) => after = Some(cursor),
            None => break,
        }
    }
    assert_eq!(seen.len(), 250);
    assert_eq!(seen.first().map(String::as_str), Some("item/000"));
    assert!(seen.windows(2).all(|w| w[0] < w[1]));
}
