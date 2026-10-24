//! A window's LocalDB request never reaches the agent's own settings: a member's grants
//! (`agent:` keys, kernel/apps/settings.rs) or the updater's auto-install choice.

use super::*;
use crate::kernel::apps::settings::PRIVATE_NETWORK_KEY;
use crate::kernel::requests::handle_request;
use crate::kernel::updates::AUTO_INSTALL_KEY;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::in_memory::InMemoryInterface;
use citadel_internal_service_types::LocalDBGetAllKVSuccess;
use citadel_sdk::prelude::StackedRatchet;
use uuid::Uuid;

type Svc = CitadelWorkspaceService<InMemoryInterface, StackedRatchet>;

const ORDINARY: &str = "auto-reconnect";

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
            value: vec![1],
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
async fn a_window_cannot_read_write_or_delete_the_agents_settings() {
    let transfers = tempfile::tempdir().expect("temp dir");
    let (_connector, svc): (_, Svc) = CitadelWorkspaceService::new_in_memory(
        crate::SERVER_RECONNECT,
        crate::BrowserTransferRoot::at(transfers.path().to_path_buf()),
    );
    for key in [PRIVATE_NETWORK_KEY, AUTO_INSTALL_KEY, "agent:anything"] {
        for request in requests(key) {
            let handled = handle_request(&svc, Uuid::new_v4(), request)
                .await
                .expect("answered");
            assert_eq!(
                failure_message(&handled.response),
                Some(AGENT_SETTINGS_REFUSED),
                "{key}: {:?}",
                handled.response
            );
        }
    }
}

#[test]
fn only_the_agents_own_keys_are_refused() {
    // The control: the refusal above is the key's, not the request's. (An ordinary key goes
    // on to its handler, which needs a running node, so the decision is asserted here.)
    for request in requests(ORDINARY) {
        assert_eq!(names_reserved_key(&request), None);
    }
    for request in requests("agentless") {
        assert_eq!(names_reserved_key(&request), None);
    }
    for key in [PRIVATE_NETWORK_KEY, AUTO_INSTALL_KEY] {
        for request in requests(key) {
            assert_eq!(names_reserved_key(&request), Some(AGENT_SETTINGS_REFUSED));
        }
    }
}

#[test]
fn a_windows_get_all_omits_the_agents_settings_and_a_clear_all_keeps_them() {
    let stored: HashMap<String, Vec<u8>> = [
        (PRIVATE_NETWORK_KEY.to_string(), vec![1]),
        (AUTO_INSTALL_KEY.to_string(), vec![0]),
        (ORDINARY.to_string(), b"1".to_vec()),
    ]
    .into();
    assert_eq!(keys_a_window_clears(&stored), vec![ORDINARY.to_string()]);
    let handled = HandledRequestResult {
        response: InternalServiceResponse::LocalDBGetAllKVSuccess(LocalDBGetAllKVSuccess {
            cid: 0,
            peer_cid: None,
            map: stored,
            request_id: None,
        }),
        uuid: Uuid::nil(),
    };
    let InternalServiceResponse::LocalDBGetAllKVSuccess(shown) =
        hide_reserved_keys(handled).response
    else {
        panic!("not a GetAll answer")
    };
    assert_eq!(shown.map.keys().collect::<Vec<_>>(), vec![ORDINARY]);
}
