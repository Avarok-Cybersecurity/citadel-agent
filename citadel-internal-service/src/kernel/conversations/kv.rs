//! Where the conversation store's records live: the agent's LocalDB, bucket 0,
//! under the keys the web UI has always used. Behind a trait so the store's
//! algorithms are tested over a map.

use crate::kernel::ilm::HostIo;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, KEY_NOT_FOUND,
};
use futures::future::BoxFuture;
use std::sync::Arc;
use uuid::Uuid;

pub(crate) type KvResult<T> = Result<T, String>;

pub(crate) trait ConversationKv: Send + Sync {
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, KvResult<Option<Vec<u8>>>>;
    fn set<'a>(&'a self, key: &'a str, value: Vec<u8>) -> BoxFuture<'a, KvResult<()>>;
    fn delete<'a>(&'a self, key: &'a str) -> BoxFuture<'a, KvResult<()>>;
    fn keys(&self) -> BoxFuture<'_, KvResult<Vec<String>>>;
}

/// LocalDB bucket 0 through the agent's own handlers (`HostIo::local_db`).
pub(crate) struct HostKv(pub(crate) Arc<dyn HostIo>);

impl ConversationKv for HostKv {
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, KvResult<Option<Vec<u8>>>> {
        Box::pin(async move {
            let request = InternalServiceRequest::LocalDBGetKV {
                request_id: Uuid::new_v4(),
                cid: 0,
                peer_cid: None,
                key: key.to_string(),
            };
            match self.0.local_db(request).await {
                InternalServiceResponse::LocalDBGetKVSuccess(ok) => Ok(Some(ok.value)),
                InternalServiceResponse::LocalDBGetKVFailure(f) if f.message == KEY_NOT_FOUND => {
                    Ok(None)
                }
                InternalServiceResponse::LocalDBGetKVFailure(f) => Err(f.message),
                other => Err(format!("unexpected answer reading {key}: {other:?}")),
            }
        })
    }

    fn set<'a>(&'a self, key: &'a str, value: Vec<u8>) -> BoxFuture<'a, KvResult<()>> {
        Box::pin(async move {
            let request = InternalServiceRequest::LocalDBSetKV {
                request_id: Uuid::new_v4(),
                cid: 0,
                peer_cid: None,
                key: key.to_string(),
                value,
            };
            match self.0.local_db(request).await {
                InternalServiceResponse::LocalDBSetKVSuccess(_) => Ok(()),
                other => Err(format!("writing {key} failed: {other:?}")),
            }
        })
    }

    fn delete<'a>(&'a self, key: &'a str) -> BoxFuture<'a, KvResult<()>> {
        Box::pin(async move {
            let request = InternalServiceRequest::LocalDBDeleteKV {
                request_id: Uuid::new_v4(),
                cid: 0,
                peer_cid: None,
                key: key.to_string(),
            };
            match self.0.local_db(request).await {
                InternalServiceResponse::LocalDBDeleteKVSuccess(_) => Ok(()),
                InternalServiceResponse::LocalDBDeleteKVFailure(f)
                    if f.message.contains(KEY_NOT_FOUND) =>
                {
                    Ok(())
                }
                other => Err(format!("deleting {key} failed: {other:?}")),
            }
        })
    }

    fn keys(&self) -> BoxFuture<'_, KvResult<Vec<String>>> {
        Box::pin(async move {
            let request = InternalServiceRequest::LocalDBGetAllKV {
                request_id: Uuid::new_v4(),
                cid: 0,
                peer_cid: None,
            };
            match self.0.local_db(request).await {
                InternalServiceResponse::LocalDBGetAllKVSuccess(ok) => {
                    Ok(ok.map.into_keys().collect())
                }
                other => Err(format!("listing keys failed: {other:?}")),
            }
        })
    }
}

/// A store in memory, for tests.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct MemoryKv(
    pub(crate) parking_lot::Mutex<std::collections::BTreeMap<String, Vec<u8>>>,
);

#[cfg(test)]
impl ConversationKv for MemoryKv {
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, KvResult<Option<Vec<u8>>>> {
        let value = self.0.lock().get(key).cloned();
        Box::pin(async move { Ok(value) })
    }
    fn set<'a>(&'a self, key: &'a str, value: Vec<u8>) -> BoxFuture<'a, KvResult<()>> {
        self.0.lock().insert(key.to_string(), value);
        Box::pin(async { Ok(()) })
    }
    fn delete<'a>(&'a self, key: &'a str) -> BoxFuture<'a, KvResult<()>> {
        self.0.lock().remove(key);
        Box::pin(async { Ok(()) })
    }
    fn keys(&self) -> BoxFuture<'_, KvResult<Vec<String>>> {
        let keys = self.0.lock().keys().cloned().collect();
        Box::pin(async move { Ok(keys) })
    }
}
