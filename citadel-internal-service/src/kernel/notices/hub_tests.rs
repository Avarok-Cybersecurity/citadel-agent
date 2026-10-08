use super::*;
use citadel_internal_service_types::{NoticeKind, NoticeTarget};
use parking_lot::Mutex;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

#[derive(Default)]
struct Recorder(Mutex<Vec<String>>);

impl Notifier for Recorder {
    fn notify(&self, notice: &NativeNotice) {
        self.0.lock().push(notice.title.clone());
    }
    fn rows(&self, rows: &NoticeRows) {
        self.0.lock().push(format!("{} rows", rows.rows.len()));
    }
}

fn notice() -> NativeNotice {
    NativeNotice {
        cid: 7,
        kind: NoticeKind::Message,
        title: "bob".into(),
        body: "New message".into(),
        target: NoticeTarget {
            account: "alice".into(),
            server_host: None,
            open: "conversation:9".into(),
        },
        request_id: None,
    }
}

fn window(clients: &Clients) -> (Uuid, UnboundedReceiver<InternalServiceResponse>) {
    let (tx, rx) = unbounded_channel();
    let id = Uuid::new_v4();
    clients.write().insert(id, tx);
    (id, rx)
}

fn hub(token: &str) -> (NoticeHub, Clients) {
    let clients: Clients = Arc::default();
    (
        NoticeHub::new(NoticeToken::new(token.into()), clients.clone(), Vec::new()),
        clients,
    )
}

#[test]
fn only_the_launch_token_opens_the_notice_plane() {
    let (with, _) = hub("s3cret-launch-token");
    assert!(with.admits("s3cret-launch-token"));
    assert!(!with.admits("s3cret-launch-tokeN"));
    assert!(!with.admits(""));
    let (without, _) = hub("");
    assert!(
        !without.admits(""),
        "an empty token must not admit an empty guess"
    );
    assert!(NoticeToken::new(String::new()).is_none());
}

#[test]
fn the_token_never_prints() {
    let token = NoticeToken::new("s3cret-launch-token".into()).unwrap();
    assert!(!format!("{token:?}").contains("s3cret"));
}

#[test]
fn notices_reach_subscribers_and_no_other_connection() {
    let (hub, clients) = hub("t");
    let (app, mut app_rx) = window(&clients);
    let (_page, mut page_rx) = window(&clients);
    hub.subscribe(app);
    hub.send_notice(&notice());
    assert!(matches!(
        app_rx.try_recv(),
        Ok(InternalServiceResponse::NativeNotice(_))
    ));
    assert!(
        page_rx.try_recv().is_err(),
        "a page that did not subscribe heard a notice"
    );
    clients.write().remove(&app);
    assert!(!hub.is_heard(), "a closed app still counted as listening");
    hub.send_notice(&notice());
    assert!(app_rx.try_recv().is_err());
}

#[test]
fn every_notifier_hears_every_notice_and_row() {
    let recorder: Arc<Recorder> = Arc::default();
    let other: Arc<dyn Notifier> = recorder.clone();
    let hub = NoticeHub::new(None, Arc::default(), vec![other]);
    assert!(hub.is_heard());
    hub.send_notice(&notice());
    hub.send_rows(&NoticeRows {
        cid: 0,
        rows: Vec::new(),
        request_id: None,
    });
    assert_eq!(
        *recorder.0.lock(),
        vec!["bob".to_string(), "0 rows".to_string()]
    );
}

#[test]
fn focus_is_per_window_and_leaves_with_it() {
    let (hub, clients) = hub("t");
    let ((a, _a_rx), (b, _b_rx)) = (window(&clients), window(&clients));
    hub.report_focus(a, 7, Some(9), true);
    hub.report_focus(b, 7, None, true);
    assert_eq!(hub.focused_on(7).len(), 2);
    assert!(hub.focused_on(8).is_empty());
    hub.report_focus(a, 7, None, false);
    assert_eq!(hub.focused_on(7), vec![None]);
    clients.write().remove(&b);
    assert!(
        hub.focused_on(7).is_empty(),
        "a closed window still held focus"
    );
}

/// The windows are told when whether anything hears notices changes, and only then.
#[test]
fn a_change_in_who_hears_is_reported_once() {
    let (hub, clients) = hub("token");
    assert_eq!(hub.heard_changed(), None, "nothing heard, nothing told");
    let (app, rx) = window(&clients);
    hub.subscribe(app);
    assert_eq!(hub.heard_changed(), Some(true));
    assert_eq!(hub.heard_changed(), None, "said once");
    clients.write().remove(&app);
    drop(rx);
    assert_eq!(
        hub.heard_changed(),
        Some(false),
        "the app's connection went away"
    );
    assert_eq!(hub.heard_changed(), None);
}
