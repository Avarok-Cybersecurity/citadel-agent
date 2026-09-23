pub mod delete_virtual_file;
pub mod download;
pub mod pick_file;
pub mod respond_file_transfer;
pub mod upload;

use citadel_sdk::prelude::{NodeResult, Ratchet};

/// Why the SDK refused a file request, if this node result is a refusal.
///
/// A refusal arrives in one of two forms: `InternalServerError` when the request
/// is gated before it leaves (no filesystem backend on one end: "Both nodes must
/// use a filesystem backend"), or a RE-VFS result carrying an error when the
/// other side could not do it (a pull of a path that does not exist). SendFile
/// and DownloadFile watch their ticket for either, so neither is left with the
/// client told only that the request was sent.
/// How long a transfer request waits for its subscription's first event: the SDK's refusal of
/// the request, or its first tick. The client was told the transfer is queued; without a bound, a
/// subscription that never yields holds its task forever. Same budget as PEER_SEND_TIMEOUT.
pub(crate) const FIRST_EVENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

pub(crate) fn refusal<R: Ratchet>(result: &NodeResult<R>) -> Option<String> {
    match result {
        NodeResult::InternalServerError(err) => Some(err.message.clone()),
        NodeResult::ReVFS(revfs) => revfs.error_message.clone(),
        _ => None,
    }
}
