//! An app's `local` storage over the account's LocalDB, one namespace per app, scope and
//! account (namespace.rs), with its limits enforced here, in the agent.
//!
//! Each namespace keeps an index (every key and its value's size), so a quota check and a
//! listing never scan the account's whole store. Writes to one namespace are serialised. The
//! order of the two writes never under-counts: a set records the new size before the value, a
//! delete removes the value before the index entry, so an interrupted write leaves the index
//! over the true usage, never under it.

mod namespace;

#[cfg(test)]
mod tests;

pub(crate) use namespace::{Namespace, APP_STORAGE_PREFIX};

use crate::kernel::conversations::kv::ConversationKv;
use citadel_internal_service_types::{
    AppKvOp, AppKvRefusal, AppKvResult, APP_KV_ENTRY_OVERHEAD_BYTES, APP_KV_LIST_PAGE,
    APP_KV_MAX_TOTAL_BYTES, APP_KV_MAX_VALUE_BYTES,
};
use citadel_sdk::logging::warn;
use parking_lot::Mutex;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// Every key in a namespace, and the length of its value.
type Index = BTreeMap<String, u64>;

fn cost(key: &str, value_len: u64) -> u64 {
    key.len() as u64 + value_len + APP_KV_ENTRY_OVERHEAD_BYTES
}

fn storage(what: &str, why: String) -> AppKvRefusal {
    warn!(target: "citadel", "[APP-KV] {what}: {why}");
    AppKvRefusal::Storage
}

/// The per-namespace write locks.
#[derive(Default)]
pub(crate) struct AppKv {
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl AppKv {
    pub(crate) async fn run(
        &self,
        store: &dyn ConversationKv,
        ns: &Namespace,
        op: AppKvOp,
    ) -> Result<AppKvResult, AppKvRefusal> {
        match op {
            AppKvOp::Get { key } => {
                let entry = ns.entry(&key)?;
                let value = store.get(&entry).await.map_err(|why| storage("get", why))?;
                Ok(AppKvResult::Value(value))
            }
            AppKvOp::Set { key, value } => {
                let entry = ns.entry(&key)?;
                if value.len() > APP_KV_MAX_VALUE_BYTES {
                    return Err(AppKvRefusal::Value);
                }
                let lock = self.lock_for(ns);
                let _held = lock.lock().await;
                let mut index = read_index(store, ns).await?;
                let used: u64 = index.iter().map(|(k, len)| cost(k, *len)).sum();
                let replaced = index.get(&key).map_or(0, |len| cost(&key, *len));
                let after = used - replaced + cost(&key, value.len() as u64);
                if after > APP_KV_MAX_TOTAL_BYTES {
                    return Err(AppKvRefusal::Quota);
                }
                index.insert(key, value.len() as u64);
                write_index(store, ns, &index).await?;
                store
                    .set(&entry, value)
                    .await
                    .map_err(|why| storage("set", why))?;
                Ok(AppKvResult::Done)
            }
            AppKvOp::Delete { key } => {
                let entry = ns.entry(&key)?;
                let lock = self.lock_for(ns);
                let _held = lock.lock().await;
                store
                    .delete(&entry)
                    .await
                    .map_err(|why| storage("delete", why))?;
                let mut index = read_index(store, ns).await?;
                if index.remove(&key).is_some() {
                    write_index(store, ns, &index).await?;
                }
                Ok(AppKvResult::Done)
            }
            AppKvOp::List { prefix, after } => {
                let index = read_index(store, ns).await?;
                let prefix = prefix.unwrap_or_default();
                let mut matching = index
                    .keys()
                    .filter(|k| k.starts_with(&prefix))
                    .filter(|k| after.as_ref().is_none_or(|a| k.as_str() > a.as_str()));
                let keys: Vec<String> = matching.by_ref().take(APP_KV_LIST_PAGE).cloned().collect();
                let next = match matching.next() {
                    Some(_) => keys.last().cloned(),
                    None => None,
                };
                Ok(AppKvResult::Keys { keys, next })
            }
        }
    }

    fn lock_for(&self, ns: &Namespace) -> Arc<tokio::sync::Mutex<()>> {
        self.locks.lock().entry(ns.index()).or_default().clone()
    }
}

async fn read_index(store: &dyn ConversationKv, ns: &Namespace) -> Result<Index, AppKvRefusal> {
    match store.get(&ns.index()).await {
        Ok(None) => Ok(Index::new()),
        Ok(Some(bytes)) => serde_json::from_slice(&bytes)
            .map_err(|why| storage("index unreadable", why.to_string())),
        Err(why) => Err(storage("index", why)),
    }
}

async fn write_index(
    store: &dyn ConversationKv,
    ns: &Namespace,
    index: &Index,
) -> Result<(), AppKvRefusal> {
    let bytes = serde_json::to_vec(index).map_err(|why| storage("index", why.to_string()))?;
    store
        .set(&ns.index(), bytes)
        .await
        .map_err(|why| storage("index write", why))
}
