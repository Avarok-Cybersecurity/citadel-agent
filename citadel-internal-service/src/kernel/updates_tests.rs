use super::*;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use uuid::Uuid;

fn window(clients: &Clients) -> (Uuid, UnboundedReceiver<InternalServiceResponse>) {
    let (tx, rx) = unbounded_channel();
    let id = Uuid::new_v4();
    clients.write().insert(id, tx);
    (id, rx)
}

fn update() -> UpdateAvailable {
    UpdateAvailable {
        cid: 0,
        current: "0.8.8".into(),
        latest: "0.9.0".into(),
        notes_url: "n".into(),
        download_url: "d".into(),
        ready: true,
        mldsa_verified: true,
        request_id: None,
    }
}

#[test]
fn every_window_hears_an_update_signed_in_or_not() {
    let clients: Clients = Arc::default();
    let (_, mut a) = window(&clients);
    let (_, mut b) = window(&clients);
    Everyone(clients).announce(&update());
    for rx in [&mut a, &mut b] {
        assert!(
            matches!(rx.try_recv(), Ok(InternalServiceResponse::UpdateAvailable(u)) if u == update())
        );
    }
}

/// The owned request's store: `UpdateSetSettings` writes the reserved key through the agent's
/// own bucket, which a window's `LocalDBSetKV` cannot name (requests/local_db).
#[tokio::test]
async fn the_owned_setting_is_written_under_the_reserved_key() {
    let settings = KvSettings(Box::new(
        crate::kernel::conversations::kv::MemoryKv::default(),
    ));
    assert_eq!(settings.auto_install().await, Ok(None));
    settings.set_auto_install(false).await.expect("saved");
    assert_eq!(settings.auto_install().await, Ok(Some(false)));
    assert_eq!(
        crate::kernel::requests::local_db::reserved_key(AUTO_INSTALL_KEY),
        Some(crate::kernel::requests::local_db::AGENT_SETTINGS_REFUSED)
    );
}
