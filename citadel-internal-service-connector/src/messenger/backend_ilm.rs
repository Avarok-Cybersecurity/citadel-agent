//! ILM's `Backend` for `CitadelWorkspaceBackend`, over any `IlmKvStore`.
//!
//! Split out of backend.rs verbatim when storage became a trait; only the four
//! key/value methods changed, and only to hand their fully-formed keys to the
//! store instead of building LocalDB requests themselves.
use crate::messenger::backend::{
    CitadelWorkspaceBackend, INBOUND_MESSAGE_PREFIX, OUTBOUND_MESSAGE_PREFIX,
};
use crate::messenger::backend_map::mutate;
use crate::messenger::kv_store::IlmKvStore;
use crate::messenger::{sleep_internal, WrappedMessage};
use async_trait::async_trait;
use citadel_internal_service_types::InternalServicePayload;
use intersession_layer_messaging::{Backend, BackendError};
use std::collections::HashMap;
use std::time::Duration;
use uuid::Uuid;

#[async_trait]
impl<S: IlmKvStore> Backend<WrappedMessage> for CitadelWorkspaceBackend<S> {
    async fn store_outbound(
        &self,
        message: WrappedMessage,
    ) -> Result<(), BackendError<WrappedMessage>> {
        let message_id = message.message_id;
        let peer_cid = message.destination_id;
        let request_id = if let InternalServicePayload::Request(request) = &message.contents {
            request.request_id().copied().unwrap_or_default()
        } else {
            Uuid::new_v4()
        };

        citadel_logging::debug!(target: "citadel", "[STORE_OUTBOUND] Storing outbound message: source_id={}, destination_id={}, message_id={}",
            message.source_id, message.destination_id, message.message_id);

        mutate(
            self,
            &self.outbound_gate,
            OUTBOUND_MESSAGE_PREFIX,
            request_id,
            move |outbound| {
                outbound
                    .entry(peer_cid)
                    .or_insert_with(HashMap::new)
                    .insert(message_id, message);
            },
        )
        .await
    }

    async fn store_inbound(
        &self,
        message: WrappedMessage,
    ) -> Result<(), BackendError<WrappedMessage>> {
        let message_id = message.message_id;
        let peer_cid = message.source_id; // Use source_id for inbound messages
        let request_id = if let InternalServicePayload::Request(request) = &message.contents {
            request.request_id().copied().unwrap_or_default()
        } else {
            Uuid::new_v4()
        };

        citadel_logging::debug!(target: "citadel", "[STORE_INBOUND] Storing inbound message: source_id={}, destination_id={}, message_id={}",
            message.source_id, message.destination_id, message.message_id);

        mutate(
            self,
            &self.inbound_gate,
            INBOUND_MESSAGE_PREFIX,
            request_id,
            move |inbound| {
                inbound
                    .entry(peer_cid)
                    .or_insert_with(HashMap::new)
                    .insert(message_id, message);
            },
        )
        .await
    }

    async fn clear_message_inbound(
        &self,
        peer_id: u64,
        message_id: u64,
    ) -> Result<(), BackendError<WrappedMessage>> {
        mutate(
            self,
            &self.inbound_gate,
            INBOUND_MESSAGE_PREFIX,
            Uuid::new_v4(),
            move |inbound| {
                if let Some(peer_messages) = inbound.get_mut(&peer_id) {
                    peer_messages.remove(&message_id);
                }
            },
        )
        .await
    }

    async fn clear_message_outbound(
        &self,
        peer_id: u64,
        message_id: u64,
    ) -> Result<(), BackendError<WrappedMessage>> {
        mutate(
            self,
            &self.outbound_gate,
            OUTBOUND_MESSAGE_PREFIX,
            Uuid::new_v4(),
            move |outbound| {
                if let Some(peer_messages) = outbound.get_mut(&peer_id) {
                    peer_messages.remove(&message_id);
                }
            },
        )
        .await
    }

    /// One read-modify-write for the whole set.
    ///
    /// Acknowledgement is cumulative, so a single ACK routinely retires a whole
    /// send window. Clearing them one at a time meant a full queue read AND a
    /// full queue write per covered id: O(window) round trips to the agent and
    /// O(window^2) bytes serialised, for one ACK.
    async fn clear_messages_outbound(
        &self,
        peer_id: u64,
        message_ids: &[u64],
    ) -> Result<(), BackendError<WrappedMessage>> {
        if message_ids.is_empty() {
            return Ok(());
        }
        let message_ids = message_ids.to_vec();
        mutate(
            self,
            &self.outbound_gate,
            OUTBOUND_MESSAGE_PREFIX,
            Uuid::new_v4(),
            move |outbound| {
                if let Some(peer_messages) = outbound.get_mut(&peer_id) {
                    for message_id in &message_ids {
                        peer_messages.remove(message_id);
                    }
                }
            },
        )
        .await
    }

    async fn get_pending_outbound(
        &self,
    ) -> Result<Vec<WrappedMessage>, BackendError<WrappedMessage>> {
        loop {
            match self.get_map(OUTBOUND_MESSAGE_PREFIX).await {
                Ok(outbound) => {
                    return Ok(outbound
                        .values()
                        .flat_map(|messages| messages.values().cloned())
                        .collect())
                }
                Err(e) => {
                    // If we get a delivery error, log it and return an empty vector
                    let err_str = format!("{e:?}");
                    if err_str.contains("Failed to deliver message")
                        || err_str.contains("get_kv: Server connection not found")
                    {
                        citadel_logging::warn!(target: "citadel", "[GET_PENDING_OUTBOUND] Failed to get outbound map due to likely no connection up yet");
                        sleep_internal(Duration::from_millis(5000)).await;
                        continue;
                    } else {
                        return Err(e);
                    }
                }
            }
        }
    }

    async fn get_pending_inbound(
        &self,
    ) -> Result<Vec<WrappedMessage>, BackendError<WrappedMessage>> {
        loop {
            match self.get_map(INBOUND_MESSAGE_PREFIX).await {
                Ok(inbound) => {
                    return Ok(inbound
                        .values()
                        .flat_map(|messages| messages.values().cloned())
                        .collect())
                }
                Err(e) => {
                    // If we get a delivery error, log it and return an empty vector
                    let err_str = format!("{e:?}");
                    if err_str.contains("Failed to deliver message")
                        || err_str.contains("get_kv: Server connection not found")
                    {
                        citadel_logging::warn!(target: "citadel", "[GET_PENDING_INBOUND] Failed to get inbound map likely due to likely no connection up yet");
                        sleep_internal(Duration::from_millis(5000)).await;
                        continue;
                    } else {
                        return Err(e);
                    }
                }
            }
        }
    }

    async fn store_value(
        &self,
        key: &str,
        value: &[u8],
    ) -> Result<(), BackendError<WrappedMessage>> {
        self.store
            .set(Uuid::new_v4(), &self.storage_key(key), value.to_vec())
            .await
            .inspect(|_| {
                citadel_logging::debug!(target: "citadel", "[STORE_VALUE] Stored value for key={}", key);
            })
    }

    /// This is how the delivery frontier and the next-id counter are read. A
    /// read that failed, reported as "nothing stored", restarts the counter and
    /// re-delivers messages the peer has already seen -- so the store's
    /// three-way answer (value / absent / error) is passed through untouched.
    async fn load_value(&self, key: &str) -> Result<Option<Vec<u8>>, BackendError<WrappedMessage>> {
        let value = self.store.get(&self.storage_key(key)).await?;
        citadel_logging::debug!(target: "citadel", "[LOAD_VALUE] Loaded value for key={}", key);
        Ok(value)
    }

    async fn load_values_batched(
        &self,
        keys: &[&str],
    ) -> Result<Vec<Option<Vec<u8>>>, BackendError<WrappedMessage>> {
        let keys: Vec<String> = keys.iter().map(|key| self.storage_key(key)).collect();
        self.store.get_many(&keys).await
    }

    /// One round trip for the whole set, mirroring `load_values_batched`.
    ///
    /// The inbound path writes the receipt map and the per-peer high-water mark
    /// for every arriving message, inline in the single sequential listener.
    /// Two separate `store_value` calls meant two round trips to the agent per
    /// message, each with its own five-second `wait_for_response` window in
    /// which one lost response freezes ALL inbound processing -- ACKs included,
    /// so the senders start retransmitting into a receiver that is not reading.
    async fn store_values_batched(
        &self,
        entries: &[(&str, Vec<u8>)],
    ) -> Result<(), BackendError<WrappedMessage>> {
        let entries: Vec<(String, Vec<u8>)> = entries
            .iter()
            .map(|(key, value)| (self.storage_key(key), value.clone()))
            .collect();
        self.store.set_many(&entries).await
    }
}
