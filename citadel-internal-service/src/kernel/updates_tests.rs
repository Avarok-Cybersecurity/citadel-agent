use super::*;
use crate::kernel::notices::NoticeToken;
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

#[test]
fn only_the_menu_bar_app_is_handed_the_image_and_without_it_nothing_installs() {
    let clients: Clients = Arc::default();
    let hub = Arc::new(NoticeHub::new(
        NoticeToken::new("t0ken".into()),
        clients.clone(),
        Vec::new(),
    ));
    let handoff = MacAppHandoff(hub.clone());
    let (_, mut page) = window(&clients);
    let version = Version::new(0, 9, 0);
    assert!(handoff.can_install().is_err());
    assert!(handoff
        .install(Path::new("/c/Citadel-Agent.dmg"), &version)
        .is_err());

    let (app_id, mut app) = window(&clients);
    hub.subscribe(app_id);
    assert!(handoff.can_install().is_ok());
    handoff
        .install(Path::new("/c/Citadel-Agent.dmg"), &version)
        .unwrap();
    match app.try_recv() {
        Ok(InternalServiceResponse::UpdateInstall(i)) => {
            assert_eq!(
                (i.version.as_str(), i.path.as_str()),
                ("0.9.0", "/c/Citadel-Agent.dmg")
            )
        }
        other => panic!("the app got {other:?}"),
    }
    assert!(
        page.try_recv().is_err(),
        "a page is never handed the install"
    );
}
