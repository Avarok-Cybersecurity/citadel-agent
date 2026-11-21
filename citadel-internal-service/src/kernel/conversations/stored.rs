//! The bytes a conversation page or its metadata occupies in LocalDB: UTF-8 JSON
//! of the canonical types (citadel-internal-service-types `conversation.rs`), as
//! serde writes them. The agent is the store's only writer and reader.

use citadel_internal_service_types::{ConversationMetadata, ConversationPage};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct StoredError(pub String);

fn err(e: impl std::fmt::Display) -> StoredError {
    StoredError(e.to_string())
}

pub(crate) fn encode_page(page: &ConversationPage) -> Result<Vec<u8>, StoredError> {
    serde_json::to_vec(page).map_err(err)
}

pub(crate) fn decode_page(bytes: &[u8]) -> Result<ConversationPage, StoredError> {
    serde_json::from_slice(bytes).map_err(err)
}

pub(crate) fn encode_metadata(metadata: &ConversationMetadata) -> Result<Vec<u8>, StoredError> {
    serde_json::to_vec(metadata).map_err(err)
}

pub(crate) fn decode_metadata(bytes: &[u8]) -> Result<ConversationMetadata, StoredError> {
    serde_json::from_slice(bytes).map_err(err)
}

#[cfg(test)]
#[path = "stored_tests.rs"]
pub(crate) mod tests;
