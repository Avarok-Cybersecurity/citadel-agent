//! A window's LocalDB request never reaches an app's `local` records.

use super::*;
use crate::kernel::requests::handle_request;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::in_memory::InMemoryInterface;
use citadel_internal_service_types::LocalDBGetAllKVSuccess;
use citadel_sdk::prelude::StackedRatchet;
use uuid::Uuid;

type Svc = CitadelWorkspaceService<InMemoryInterface, StackedRatchet>;

const APP_KEY: &str = "app:com.acme.kanban:6e6f64653a31:626f62:i";

fn service() -> (tempfile::TempDir, Svc) {
    let transfers = tempfile::tempdir().expect("temp dir");
    let (_connector, svc): (_, Svc) = CitadelWorkspaceService::new_in_memory(
        crate::SERVER_RECONNECT,
        crate::BrowserTransferRoot::at(transfers.path().to_path_buf()),
    );
    (transfers, svc)
}

fn requests(key: &str) -> [InternalServiceRequest; 3] {
    // cid 0, the agent's own bucket, passes the ownership gate, so only this check refuses.
    [
        InternalServiceRequest::LocalDBGetKV {
            request_id: Uuid::new_v4(),
            cid: 0,
            peer_cid: None,
            key: key.into(),
        },
        InternalServiceRequest::LocalDBSetKV {
            request_id: Uuid::new_v4(),
            cid: 0,
            peer_cid: None,
            key: key.into(),
            value: b"clobbered".to_vec(),
        },
        InternalServiceRequest::LocalDBDeleteKV {
            request_id: Uuid::new_v4(),
            cid: 0,
            peer_cid: None,
            key: key.into(),
        },
    ]
}

fn failure_message(response: &InternalServiceResponse) -> Option<&str> {
    match response {
        InternalServiceResponse::LocalDBGetKVFailure(f) => Some(&f.message),
        InternalServiceResponse::LocalDBSetKVFailure(f) => Some(&f.message),
        InternalServiceResponse::LocalDBDeleteKVFailure(f) => Some(&f.message),
        _ => None,
    }
}

#[tokio::test]
async fn a_window_cannot_read_write_or_delete_an_app_key() {
    let (_dir, svc) = service();
    for request in requests(APP_KEY) {
        let handled = handle_request(&svc, Uuid::new_v4(), request)
            .await
            .expect("answered");
        assert_eq!(
            failure_message(&handled.response),
            Some(APP_STORAGE_REFUSED),
            "{:?}",
            handled.response
        );
    }
}

#[test]
fn only_an_app_key_is_refused() {
    // The control: the refusal above is the key's, not the request's. (An ordinary key goes
    // on to its handler, which needs a running node, so the decision is asserted here.)
    for request in requests("auto-reconnect") {
        assert!(!names_app_storage(&request));
    }
    for request in requests(APP_KEY) {
        assert!(names_app_storage(&request));
    }
    assert!(!names_app_storage(&InternalServiceRequest::LocalDBGetKV {
        request_id: Uuid::new_v4(),
        cid: 0,
        peer_cid: None,
        key: "application-settings".into(),
    }));
}

#[test]
fn a_windows_get_all_omits_app_keys_and_a_clear_all_keeps_them() {
    let stored: HashMap<String, Vec<u8>> = [
        (APP_KEY.to_string(), b"index".to_vec()),
        ("auto-reconnect".to_string(), b"1".to_vec()),
    ]
    .into();
    assert_eq!(
        keys_a_window_clears(&stored),
        vec!["auto-reconnect".to_string()]
    );
    let handled = HandledRequestResult {
        response: InternalServiceResponse::LocalDBGetAllKVSuccess(LocalDBGetAllKVSuccess {
            cid: 0,
            peer_cid: None,
            map: stored,
            request_id: None,
        }),
        uuid: Uuid::nil(),
    };
    let InternalServiceResponse::LocalDBGetAllKVSuccess(shown) = hide_app_storage(handled).response
    else {
        panic!("not a GetAll answer")
    };
    assert_eq!(shown.map.keys().collect::<Vec<_>>(), vec!["auto-reconnect"]);
}
