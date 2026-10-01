//! The storage logic is the same whatever carries its requests.
//!
//! The agent hosts ILM with its own channel (answering LocalDB requests
//! in-process) where a browser uses `RequestChannel` over its socket. An
//! account's state written through one must read back through the other, so
//! the keys and encodings are pinned here through a channel that is a plain
//! map -- the shape of the agent's store, with no socket.

use super::*;
use citadel_internal_service_types::{
    LocalDBGetKVFailure, LocalDBGetKVSuccess, LocalDBSetKVSuccess,
};
use parking_lot::Mutex as SyncMutex;

/// A LocalDB as a map from the full key to its value.
#[derive(Clone, Default)]
pub(crate) struct MapChannel {
    pub(crate) store: Arc<SyncMutex<HashMap<String, Vec<u8>>>>,
}

impl MapChannel {
    fn answer(&self, request: InternalServiceRequest) -> InternalServiceResponse {
        match request {
            InternalServiceRequest::LocalDBGetKV {
                request_id,
                cid,
                peer_cid,
                key,
            } => match self.store.lock().get(&key).cloned() {
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
                self.store.lock().insert(key.clone(), value);
                InternalServiceResponse::LocalDBSetKVSuccess(LocalDBSetKVSuccess {
                    cid,
                    peer_cid,
                    key,
                    request_id: Some(request_id),
                })
            }
            InternalServiceRequest::Batched {
                request_id,
                commands,
            } => InternalServiceResponse::BatchedResponse(BatchedResponseData {
                cid: 0,
                request_id: Some(request_id),
                results: commands.into_iter().map(|c| self.answer(c)).collect(),
            }),
            other => panic!("the backend sent a request it should not: {other:?}"),
        }
    }
}

#[async_trait]
impl BackendChannel for MapChannel {
    async fn request(
        &self,
        request: InternalServiceRequest,
        _request_id: Uuid,
    ) -> Result<InternalServiceResponse, BackendError<WrappedMessage>> {
        Ok(self.answer(request))
    }
}

fn message(id: u64, to: u64) -> WrappedMessage {
    WrappedMessage {
        source_id: 7,
        destination_id: to,
        message_id: id,
        contents: citadel_internal_service_types::InternalServicePayload::Response(
            InternalServiceResponse::MessageSendSuccess(
                citadel_internal_service_types::MessageSendSuccess {
                    cid: 7,
                    peer_cid: Some(to),
                    request_id: None,
                },
            ),
        ),
    }
}

/// The keys are the agent's existing ones, `{name}-{cid}`: this is what lets an
/// agent-hosted ILM adopt the state a browser's ILM persisted.
#[citadel_io::tokio::test]
async fn values_and_maps_live_under_the_accounts_existing_keys() {
    let channel = MapChannel::default();
    let backend = CitadelWorkspaceBackend::with_channel(7, channel.clone());

    backend.store_value("last_sent", b"frontier").await.unwrap();
    backend.store_outbound(message(41, 9)).await.unwrap();
    backend.store_inbound(message(42, 7)).await.unwrap();

    let mut keys: Vec<String> = channel.store.lock().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        vec!["inbound_messages-7", "last_sent-7", "outbound_messages-7"]
    );
}

/// Written through one backend, read through another over the same store:
/// the hand-over from a browser's ILM to the agent's.
#[citadel_io::tokio::test]
async fn state_written_by_one_backend_is_read_by_another() {
    let channel = MapChannel::default();
    let writer = CitadelWorkspaceBackend::with_channel(7, channel.clone());
    writer.store_value("next_unique_id", b"\x05").await.unwrap();
    writer.store_outbound(message(41, 9)).await.unwrap();
    writer
        .store_values_batched(&[("last_acked", b"a".to_vec())])
        .await
        .unwrap();

    let reader = CitadelWorkspaceBackend::with_channel(7, channel);
    assert_eq!(
        reader.load_value("next_unique_id").await.unwrap(),
        Some(b"\x05".to_vec())
    );
    let pending = reader.get_pending_outbound().await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].message_id, 41);
    assert_eq!(
        reader
            .load_values_batched(&["last_acked", "never_written"])
            .await
            .unwrap(),
        vec![Some(b"a".to_vec()), None]
    );
}

/// Another account's keys are never touched.
#[citadel_io::tokio::test]
async fn two_accounts_do_not_share_a_key() {
    let channel = MapChannel::default();
    CitadelWorkspaceBackend::with_channel(7, channel.clone())
        .store_value("last_sent", b"seven")
        .await
        .unwrap();
    let other = CitadelWorkspaceBackend::with_channel(8, channel);
    assert_eq!(other.load_value("last_sent").await.unwrap(), None);
}
