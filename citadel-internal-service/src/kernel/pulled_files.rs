//! The file a RE-VFS pull produced, which this session may then send on.
//!
//! RE-VFS is pusher-owned: a peer holds the uploader's file encrypted for the
//! uploader, so only the uploader can ever open it. For the peer to get a
//! readable copy of a file in their shared storage, the uploader's agent pulls
//! it back and sends it to them as an ordinary transfer. A browser may only
//! name a `Path` this process handed it (`picked_files::was_picked`), and
//! nothing handed it the pull's output, so that send was refused.
//!
//! A pull's completed output is a file this agent wrote for this session, from
//! bytes only this session could decrypt. It is recorded as sendable for the
//! same window and under the same cap as a picked file, and nothing else is:
//! an unfinished or failed pull records nothing.

use super::PickedFileInfo;
use citadel_sdk::prelude::ObjectTransferStatus;
use std::path::PathBuf;
use std::time::Instant;

/// Watches one pull's tick stream for the file it completes.
#[derive(Default)]
pub struct PulledOutput {
    begun: Option<(PathBuf, String, u64)>,
}

impl PulledOutput {
    /// The finished file, once `status` completes the reception it began.
    pub fn observe(
        &mut self,
        status: &ObjectTransferStatus,
        now: Instant,
    ) -> Option<PickedFileInfo> {
        match status {
            ObjectTransferStatus::ReceptionBeginning(path, metadata) => {
                self.begun = Some((
                    path.clone(),
                    metadata.name.clone(),
                    metadata.plaintext_length as u64,
                ));
                None
            }
            ObjectTransferStatus::ReceptionComplete => {
                self.begun
                    .take()
                    .map(|(file_path, file_name, file_size)| PickedFileInfo {
                        file_path,
                        file_name,
                        file_size,
                        picked_at: now,
                    })
            }
            ObjectTransferStatus::Fail(_) => {
                self.begun = None;
                None
            }
            _ => None,
        }
    }
}

/// What the tick task calls with a pull's finished file.
pub type PulledFileHook = Box<dyn FnOnce(PickedFileInfo) + Send + 'static>;

#[cfg(test)]
mod tests {
    use super::PulledOutput;
    use crate::kernel::picked_files::{store, was_picked};
    use citadel_sdk::prelude::{ObjectTransferStatus, TransferType, VirtualObjectMetadata};
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::time::Instant;
    use uuid::Uuid;

    fn beginning(path: &str) -> ObjectTransferStatus {
        ObjectTransferStatus::ReceptionBeginning(
            PathBuf::from(path),
            VirtualObjectMetadata {
                name: "atlas.png".into(),
                date_created: String::new(),
                author: String::new(),
                plaintext_length: 1400,
                group_count: 1,
                object_id: citadel_internal_service_types::ObjectId(0),
                cid: 1,
                transfer_type: TransferType::FileTransfer,
            },
        )
    }

    #[test]
    fn a_completed_pull_becomes_sendable() {
        let now = Instant::now();
        let mut out = PulledOutput::default();
        assert!(out
            .observe(&beginning("/data/transfers/9/atlas.png"), now)
            .is_none());
        let info = out
            .observe(&ObjectTransferStatus::ReceptionComplete, now)
            .expect("finished");
        let mut picked = HashMap::new();
        store(&mut picked, Uuid::new_v4(), info, now);
        assert!(was_picked(
            &picked,
            Path::new("/data/transfers/9/atlas.png"),
            now
        ));
        assert!(
            !was_picked(&picked, Path::new("/etc/passwd"), now),
            "only the pull's own output"
        );
    }

    #[test]
    fn a_failed_or_unfinished_pull_records_nothing() {
        let now = Instant::now();
        let mut out = PulledOutput::default();
        out.observe(&beginning("/data/transfers/9/atlas.png"), now);
        assert!(out
            .observe(&ObjectTransferStatus::Fail("gone".into()), now)
            .is_none());
        assert!(out
            .observe(&ObjectTransferStatus::ReceptionComplete, now)
            .is_none());
        assert!(PulledOutput::default()
            .observe(&ObjectTransferStatus::ReceptionComplete, now)
            .is_none());
    }
}
