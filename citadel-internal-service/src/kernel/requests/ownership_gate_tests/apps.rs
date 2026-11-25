//! The apps requests act only for a session the caller is attached to.
use super::{gate_decision, requires_owned_session, GateDecision};
use citadel_internal_service_types::{
    AppHttpRequest, AppKvNamespace, AppKvOp, AppNetGrant, AppNotice, InternalServiceRequest,
};
use uuid::Uuid;

fn apps_requests(cid: u64) -> Vec<InternalServiceRequest> {
    let request_id = Uuid::new_v4();
    vec![
        InternalServiceRequest::AppFetch {
            request_id,
            cid,
            app_id: "com.acme.kanban".into(),
            install_id: "i".into(),
            grant: AppNetGrant {
                hosts: vec!["api.acme.com".into()],
                private: false,
            },
            request: AppHttpRequest {
                method: "GET".into(),
                url: "https://api.acme.com/".into(),
                headers: vec![],
                body: vec![],
            },
            stream: true,
        },
        InternalServiceRequest::AppFetchCredit {
            request_id,
            cid,
            fetch_id: request_id,
            n: 1,
        },
        InternalServiceRequest::AppFetchCancel {
            request_id,
            cid,
            fetch_id: request_id,
        },
        InternalServiceRequest::GetAppNetUsage {
            request_id,
            cid,
            install_id: "i".into(),
        },
        InternalServiceRequest::AppKv {
            request_id,
            cid,
            namespace: AppKvNamespace {
                app_id: "com.acme.kanban".into(),
                scope_key: "workspace".into(),
            },
            op: AppKvOp::Get { key: "k".into() },
        },
        InternalServiceRequest::IhttpSend {
            request_id,
            cid,
            peer_cid: 2,
            frame: vec![],
        },
        InternalServiceRequest::AppSessionOpen {
            request_id,
            cid,
            app_id: "work.synchronized.app".into(),
            install_id: "i".into(),
            acting_username: "alice@a.example".into(),
            server: "synchronized.work".into(),
        },
        InternalServiceRequest::AppSessionSend {
            request_id,
            cid,
            install_id: "i".into(),
            session: 1,
            body: vec![],
        },
        InternalServiceRequest::AppSessionClose {
            request_id,
            cid,
            install_id: "i".into(),
            session: 1,
        },
        // An account's reminders are set and cancelled by its own windows only.
        InternalServiceRequest::AppNoticePost {
            request_id,
            cid,
            notice: notice(None),
        },
        InternalServiceRequest::AppRemindersSync {
            request_id,
            cid,
            app_id: "net.avarok.calendar".into(),
            reminders: vec![notice(Some(1))],
        },
    ]
}

fn notice(fire_at_ms: Option<u64>) -> AppNotice {
    AppNotice {
        id: "reminder:e:1".into(),
        app_id: "net.avarok.calendar".into(),
        node_id: "workspace-root".into(),
        title: "t".into(),
        body: "b".into(),
        fire_at_ms,
        start_ms: None,
    }
}

#[test]
fn every_apps_request_needs_a_session_the_caller_owns() {
    for request in apps_requests(7) {
        assert_eq!(request.session_cid(), Some(7), "{request:?}");
        assert!(requires_owned_session(&request), "{request:?}");
        assert!(
            matches!(gate_decision(&request, None), GateDecision::Refuse { .. }),
            "{request:?} proceeded for an unmapped session"
        );
        assert!(
            matches!(
                gate_decision(&request, Some(false)),
                GateDecision::Refuse { .. }
            ),
            "{request:?} proceeded for another connection's session"
        );
        assert_eq!(gate_decision(&request, Some(true)), GateDecision::Proceed);
    }
}
