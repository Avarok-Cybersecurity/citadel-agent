//! Whether a call may start on a peer connection's current path. The server's path policy
//! says which paths carry media: a hosted workspace on a plan without calls through its
//! server, or a self-hosted one whose operator denied them, refuses a call that has only the
//! server relay to ride, and the window is told why rather than shown a transport error.

use crate::kernel::CitadelWorkspaceService;
use citadel_sdk::prelude::{PathClass, Ratchet};

/// The window's reason for a call refused by the server's path policy.
pub(crate) const MEDIA_PATH_DENIED: &str =
    "this workspace does not carry calls through its server (its plan or its path policy), and \
     there is no direct or TURN connection to this peer yet; the call can start once one \
     connects, and on a hosted workspace a paid plan carries calls through the server too";

/// The refusal for a path that `permits_media` or not.
pub(crate) fn media_refusal(permits_media: bool) -> Option<&'static str> {
    (!permits_media).then_some(MEDIA_PATH_DENIED)
}

/// `cid`'s connection to `peer_cid` refuses media on its current path: the reason, or `None`
/// (also when there is no such connection, which the open reports itself).
pub(super) fn media_path_refusal<T, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    peer_cid: u64,
) -> Option<&'static str> {
    media_refusal(this.peer_paths.permits(cid, peer_cid, PathClass::Media)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_without_media_is_refused_with_its_reason() {
        assert_eq!(media_refusal(false), Some(MEDIA_PATH_DENIED));
        assert!(MEDIA_PATH_DENIED.contains("does not carry calls through its server"));
        assert_eq!(media_refusal(true), None);
    }
}
