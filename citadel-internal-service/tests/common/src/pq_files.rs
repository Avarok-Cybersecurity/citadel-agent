//! RE-VFS files through a window, on a post-quantum server that keeps what it is sent, and what
//! that server holds of them: each stored object's groups start `RVF2 ‖ key id`, so a test can
//! read which key an object is sealed under without opening it.

use crate::pq_window::Window;
use crate::server_test_node_skip_cert_verification;
use citadel_internal_service_types::{FileSource, InternalServiceRequest, InternalServiceResponse};
use citadel_sdk::prelude::*;
use std::error::Error;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Accepts every object it is sent, as the workspace server's kernel does, whatever its name.
struct StoringKernel;

#[async_trait]
impl NetKernel<StackedRatchet> for StoringKernel {
    fn load_remote(&mut self, _remote: NodeRemote<StackedRatchet>) -> Result<(), NetworkError> {
        Ok(())
    }

    async fn on_start(&self) -> Result<(), NetworkError> {
        Ok(())
    }

    async fn on_node_event_received(
        &self,
        message: NodeResult<StackedRatchet>,
    ) -> Result<(), NetworkError> {
        if let NodeResult::ObjectTransferHandle(transfer) = message {
            let mut handle = transfer.handle;
            handle.accept()?;
            drop(tokio::spawn(async move {
                use futures::StreamExt;
                while handle.next().await.is_some() {}
            }));
        }
        Ok(())
    }

    async fn on_stop(&mut self) -> Result<(), NetworkError> {
        Ok(())
    }
}

/// A post-quantum server that accepts the objects it is sent, keeping its accounts and objects
/// under `store`.
pub fn spawn_storing_server(store: &Path) -> SocketAddr {
    let kernel = StoringKernel;
    let backend = BackendType::Filesystem(store.to_string_lossy().into_owned());
    let (node, addr) = server_test_node_skip_cert_verification(kernel, |builder| {
        let _ = builder.with_backend(backend);
    });
    drop(tokio::spawn(node));
    addr
}

/// The key id of the RE-VFS key `seed` gives account `cid` on `server`, as the agent derives it.
pub fn key_id(seed: &[u8; 32], server: SocketAddr, cid: u64) -> [u8; 8] {
    let domain = citadel_identity::canonical_domain(&server.to_string())
        .expect("a test server has a canonical domain");
    RevfsKey::derive(seed, domain.as_str(), cid)
        .expect("a canonical domain derives a key")
        .id()
}

/// The seed a typed phrase encodes.
pub fn seed_of(typed: &str) -> [u8; 32] {
    *RecoverySeed::parse(typed)
        .expect("a phrase the agent issued reads back")
        .as_bytes()
}

/// The key id of every sealed object under `store`, by its path there.
pub fn stored_key_ids(store: &Path) -> Vec<(PathBuf, [u8; 8])> {
    let mut found = Vec::new();
    let mut dirs = vec![store.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if let Ok(bytes) = std::fs::read(&path) {
                if bytes.len() >= 12 && &bytes[..4] == b"RVF2" {
                    let mut id = [0u8; 8];
                    id.copy_from_slice(&bytes[4..12]);
                    found.push((path, id));
                }
            }
        }
    }
    found
}

impl Window {
    /// Stores `source` at `virtual_path` on the account's server: `Ok` once the push completed.
    pub async fn store(
        &mut self,
        cid: u64,
        source: &Path,
        virtual_path: &str,
    ) -> Result<Result<(), String>, Box<dyn Error>> {
        let request_id = Uuid::new_v4();
        self.send(InternalServiceRequest::SendFile {
            request_id,
            source: FileSource::Path(source.to_path_buf()),
            cid,
            transfer_type: TransferType::RemoteEncryptedVirtualFilesystem {
                virtual_path: virtual_path.into(),
                security_level: Default::default(),
            },
            peer_cid: None,
            chunk_size: None,
        })
        .await?;
        loop {
            match self.next_of(request_id).await? {
                InternalServiceResponse::SendFileRequestFailure(failure) => {
                    return Ok(Err(failure.message))
                }
                InternalServiceResponse::FileTransferTickNotification(tick) => match tick.status {
                    ObjectTransferStatus::TransferComplete => return Ok(Ok(())),
                    ObjectTransferStatus::Fail(err) => return Ok(Err(err)),
                    _ => {}
                },
                _ => {}
            }
        }
    }

    /// Reads `virtual_path` back from the account's server: its contents, or the failure.
    pub async fn read(
        &mut self,
        cid: u64,
        virtual_path: &str,
    ) -> Result<Result<Vec<u8>, String>, Box<dyn Error>> {
        let request_id = Uuid::new_v4();
        self.send(InternalServiceRequest::DownloadFile {
            virtual_directory: virtual_path.into(),
            security_level: None,
            delete_on_pull: false,
            cid,
            peer_cid: None,
            request_id,
        })
        .await?;
        let mut saved = None;
        loop {
            match self.next_of(request_id).await? {
                InternalServiceResponse::DownloadFileFailure(failure) => {
                    return Ok(Err(failure.message))
                }
                InternalServiceResponse::FileTransferTickNotification(tick) => match tick.status {
                    ObjectTransferStatus::ReceptionBeginning(path, _) => saved = Some(path),
                    ObjectTransferStatus::ReceptionComplete => {
                        let path = saved.ok_or("the pull named no file")?;
                        return Ok(Ok(tokio::fs::read(path).await?));
                    }
                    ObjectTransferStatus::Fail(err) => return Ok(Err(err)),
                    _ => {}
                },
                _ => {}
            }
        }
    }
}
