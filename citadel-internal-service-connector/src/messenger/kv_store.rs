//! Where an ILM backend keeps its bytes.
//!
//! `CitadelWorkspaceBackend` owns every decision about WHAT is stored: the
//! `{prefix}-{cid}` keys, the bincode2 encoding of the queue maps, the gates
//! that serialise their read-modify-writes. This trait is only the storage
//! underneath, so the same backend runs in the browser (over LocalDB requests
//! on the agent's WebSocket, `backend_ws.rs`) and inside the agent (over the
//! agent's own LocalDB). Both hosts therefore write byte-identical state, and
//! an agent-hosted ILM can take over what a browser's ILM persisted.
//!
//! Keys arrive fully formed. An implementation must store them verbatim: any
//! prefixing or re-encoding here would split one account's state in two.
//!
//! The four methods are exactly the four operations the backend performs. The
//! batched pair exists because the WebSocket store sends them as ONE request
//! (`InternalServiceRequest::Batched`); a store without that cost may loop.
use crate::messenger::WrappedMessage;
use async_trait::async_trait;
use intersession_layer_messaging::BackendError;
use uuid::Uuid;

pub type KvResult<T> = Result<T, BackendError<WrappedMessage>>;

#[async_trait]
pub trait IlmKvStore: Send + Sync + 'static {
    /// `Ok(None)` means the key is absent. A failed or unanswered read is an
    /// `Err`, never `None`: absence re-initialises a queue or restarts the
    /// delivery frontier, and doing that on an error loses state silently.
    async fn get(&self, key: &str) -> KvResult<Option<Vec<u8>>>;

    /// `request_id` is the id the write travels under where the store has one
    /// (the WebSocket store's `LocalDBSetKV.request_id`). The backend passes the
    /// queued message's own request id for queue writes, as it always has.
    async fn set(&self, request_id: Uuid, key: &str, value: Vec<u8>) -> KvResult<()>;

    /// One result per key, in order.
    async fn get_many(&self, keys: &[String]) -> KvResult<Vec<Option<Vec<u8>>>>;

    /// All entries, or an error: a partial success must not report `Ok`.
    async fn set_many(&self, entries: &[(String, Vec<u8>)]) -> KvResult<()>;
}
