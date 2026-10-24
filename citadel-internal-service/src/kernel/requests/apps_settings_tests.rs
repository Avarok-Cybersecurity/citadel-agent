//! A member's private-network grant: set and read by the session's own windows, kept in that
//! account's own bucket under the reserved key, and refused to anyone else.

use super::respond;
use crate::kernel::apps::settings::{AgentAppSettings, PRIVATE_NETWORK_KEY};
use crate::kernel::ilm::HostIo;
use crate::kernel::requests::handle_request;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::in_memory::InMemoryInterface;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, LocalDBGetKVFailure, LocalDBGetKVSuccess,
    LocalDBSetKVSuccess, MessageNotification, KEY_NOT_FOUND,
};
use citadel_sdk::prelude::StackedRatchet;
use futures::future::BoxFuture;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

const ALICE: u64 = 42;

/// Every account's LocalDB, keyed by the bucket each request names. Only the two requests the
/// grant's store makes are answered.
#[derive(Default)]
struct Buckets(Mutex<HashMap<(u64, String), Vec<u8>>>);

impl Buckets {
    fn read(&self, cid: u64, key: &str) -> Option<Vec<u8>> {
        self.0.lock().get(&(cid, key.to_string())).cloned()
    }
}

impl HostIo for Buckets {
    fn local_db(
        &self,
        request: InternalServiceRequest,
    ) -> BoxFuture<'static, InternalServiceResponse> {
        let answer = match request {
            InternalServiceRequest::LocalDBGetKV {
                request_id,
                cid,
                peer_cid,
                key,
            } => match self.read(cid, &key) {
                Some(value) => InternalServiceResponse::LocalDBGetKVSuccess(LocalDBGetKVSuccess {
                    cid,
                    peer_cid,
                    key,
                    value,
                    request_id: Some(request_id),
                }),
                None => InternalServiceResponse::LocalDBGetKVFailure(LocalDBGetKVFailure {
                    cid,
                    peer_cid,
                    message: KEY_NOT_FOUND.to_string(),
                    request_id: Some(request_id),
                }),
            },
            InternalServiceRequest::LocalDBSetKV {
                request_id,
                cid,
                peer_cid,
                key,
                value,
            } => {
                self.0.lock().insert((cid, key.clone()), value);
                InternalServiceResponse::LocalDBSetKVSuccess(LocalDBSetKVSuccess {
                    cid,
                    peer_cid,
                    key,
                    request_id: Some(request_id),
                })
            }
            other => panic!("the grant's store asked for {other:?}"),
        };
        Box::pin(async move { answer })
    }
    fn send_frame(&self, _: InternalServiceRequest) -> BoxFuture<'static, Result<(), String>> {
        panic!("the grant's store sends no frame")
    }
    fn connected_peers(&self, _: u64) -> Vec<u64> {
        vec![]
    }
    fn deliver(&self, _: u64, _: MessageNotification) -> BoxFuture<'static, bool> {
        panic!("the grant's store delivers nothing")
    }
}

fn set(cid: u64, on: bool) -> InternalServiceRequest {
    InternalServiceRequest::AppsSetAgentSettings {
        request_id: Uuid::new_v4(),
        cid,
        private_network: on,
    }
}

fn get(cid: u64) -> InternalServiceRequest {
    InternalServiceRequest::AppsGetAgentSettings {
        request_id: Uuid::new_v4(),
        cid,
    }
}

fn granted(response: &InternalServiceResponse) -> Option<bool> {
    match response {
        InternalServiceResponse::AppsAgentSettings(s) => Some(s.private_network),
        _ => None,
    }
}

#[tokio::test]
async fn an_attached_window_sets_the_grant_and_reads_it_back() {
    let buckets = Arc::new(Buckets::default());
    let settings = AgentAppSettings::default();
    let unset = respond(&settings, buckets.clone(), true, get(ALICE)).await;
    assert_eq!(granted(&unset), Some(false), "{unset:?}");
    let saved = respond(&settings, buckets.clone(), true, set(ALICE, true)).await;
    assert_eq!(granted(&saved), Some(true), "{saved:?}");
    // A fresh cache reads it from the store, so it is the store's, not the memory's.
    let fresh = AgentAppSettings::default();
    let read = respond(&fresh, buckets.clone(), true, get(ALICE)).await;
    assert_eq!(granted(&read), Some(true), "{read:?}");
}

#[tokio::test]
async fn the_grant_is_kept_in_the_accounts_own_bucket_under_the_reserved_key() {
    let buckets = Arc::new(Buckets::default());
    let settings = AgentAppSettings::default();
    let _ = respond(&settings, buckets.clone(), true, set(ALICE, true)).await;
    assert_eq!(PRIVATE_NETWORK_KEY, "agent:apps_private_network");
    assert_eq!(buckets.read(ALICE, PRIVATE_NETWORK_KEY), Some(vec![1]));
    assert_eq!(
        buckets.read(0, PRIVATE_NETWORK_KEY),
        None,
        "it went to bucket 0"
    );
    let _ = respond(&settings, buckets.clone(), true, set(ALICE, false)).await;
    assert_eq!(buckets.read(ALICE, PRIVATE_NETWORK_KEY), Some(vec![0]));
}

#[tokio::test]
async fn a_caller_not_attached_to_the_session_is_refused_and_changes_nothing() {
    let buckets = Arc::new(Buckets::default());
    let settings = AgentAppSettings::default();
    let refused = respond(&settings, buckets.clone(), false, set(ALICE, true)).await;
    assert!(
        matches!(
            refused,
            InternalServiceResponse::AppsAgentSettingsFailure(_)
        ),
        "{refused:?}"
    );
    assert_eq!(buckets.read(ALICE, PRIVATE_NETWORK_KEY), None);
    let read = respond(&settings, buckets, true, get(ALICE)).await;
    assert_eq!(granted(&read), Some(false), "{read:?}");
}

/// Through the real dispatcher: a cid with no mapped session is refused by the ownership gate
/// before the handler runs, and the refusal is an answer, not silence.
#[tokio::test]
async fn the_ownership_gate_answers_a_request_for_a_session_nobody_holds() {
    let transfers = tempfile::tempdir().expect("temp dir");
    let (_connector, svc): (
        _,
        CitadelWorkspaceService<InMemoryInterface, StackedRatchet>,
    ) = CitadelWorkspaceService::new_in_memory(
        crate::SERVER_RECONNECT,
        crate::BrowserTransferRoot::at(transfers.path().to_path_buf()),
    );
    for request in [set(ALICE, true), get(ALICE)] {
        let handled = handle_request(&svc, Uuid::new_v4(), request)
            .await
            .expect("answered");
        assert!(
            matches!(
                handled.response,
                InternalServiceResponse::AppsAgentSettingsFailure(_)
            ),
            "{:?}",
            handled.response
        );
    }
}
