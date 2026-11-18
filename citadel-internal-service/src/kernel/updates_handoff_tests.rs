use super::*;
use crate::kernel::notices::NoticeToken;
use crate::kernel::session_route::Clients;
use parking_lot::Mutex;
use std::path::PathBuf;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use uuid::Uuid;

const WITHIN: Duration = Duration::from_secs(60);

fn window(clients: &Clients) -> (Uuid, UnboundedReceiver<InternalServiceResponse>) {
    let (tx, rx) = unbounded_channel();
    let id = Uuid::new_v4();
    clients.write().insert(id, tx);
    (id, rx)
}

fn dmg() -> Staged {
    Staged {
        path: PathBuf::from("/c/Citadel-Agent.dmg"),
        launch: None,
    }
}

/// A hub, an installer over it, and what the installer recorded as not taken.
fn handoff() -> (
    Clients,
    Arc<NoticeHub>,
    MacAppHandoff,
    Arc<Mutex<Vec<Version>>>,
) {
    let clients: Clients = Arc::default();
    let hub = Arc::new(NoticeHub::new(
        NoticeToken::new("t0ken".into()),
        clients.clone(),
        Vec::new(),
    ));
    let seen: Arc<Mutex<Vec<Version>>> = Arc::default();
    let record = seen.clone();
    let on_not_taken: OnNotTaken = Arc::new(move |v: &Version| record.lock().push(v.clone()));
    let installer = MacAppHandoff::new(hub.clone(), WITHIN, on_not_taken);
    (clients, hub, installer, seen)
}

#[tokio::test]
async fn only_the_menu_bar_app_is_handed_the_image_and_without_it_nothing_installs() {
    let (clients, hub, handoff, _) = handoff();
    let (_, mut page) = window(&clients);
    let version = Version::new(0, 9, 0);
    assert!(handoff.can_install(&dmg()).is_err());
    assert!(handoff.install(&dmg(), &version).is_err());

    let (app_id, mut app) = window(&clients);
    hub.subscribe(app_id);
    assert!(handoff.can_install(&dmg()).is_ok());
    handoff.install(&dmg(), &version).unwrap();
    match app.try_recv() {
        Ok(InternalServiceResponse::UpdateInstall(i)) => assert_eq!(
            (i.version.as_str(), i.path.as_str()),
            ("0.9.0", "/c/Citadel-Agent.dmg")
        ),
        other => panic!("the app got {other:?}"),
    }
    assert!(
        page.try_recv().is_err(),
        "a page is never handed the install"
    );
}

/// The app is still listed as subscribed, but its stream is gone: listed is not reached.
#[tokio::test]
async fn a_handoff_that_reaches_no_one_is_a_failure_even_while_the_app_is_listed() {
    let (clients, hub, handoff, seen) = handoff();
    let (app_id, app) = window(&clients);
    hub.subscribe(app_id);
    drop(app);
    assert!(handoff.can_install(&dmg()).is_ok(), "still listed");
    assert_eq!(
        handoff.install(&dmg(), &Version::new(0, 13, 0)),
        Err(NOT_LISTENING.to_string())
    );
    assert!(seen.lock().is_empty(), "nothing was handed over to watch");
}

#[tokio::test(start_paused = true)]
async fn an_agent_still_running_after_the_window_records_that_the_app_did_not_take_it() {
    let (clients, hub, handoff, seen) = handoff();
    let (app_id, _app) = window(&clients);
    hub.subscribe(app_id);
    handoff.install(&dmg(), &Version::new(0, 13, 0)).unwrap();

    tokio::time::sleep(WITHIN - Duration::from_secs(1)).await;
    assert!(seen.lock().is_empty(), "the app still has time to stop it");

    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(*seen.lock(), vec![Version::new(0, 13, 0)]);
}

#[test]
fn the_record_names_both_versions_and_the_way_out() {
    let why = not_taken(&Version::new(0, 13, 0), "0.11.0", WITHIN);
    assert!(
        why.contains("0.13.0") && why.contains("still 0.11.0"),
        "{why}"
    );
    assert!(
        why.contains("within 60s") && why.contains("Download 0.13.0"),
        "{why}"
    );
}
