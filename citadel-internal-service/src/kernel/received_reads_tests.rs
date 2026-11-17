use super::*;
use crate::kernel::conversations::kv::MemoryKv;
use crate::kernel::received_files;
use parking_lot::Mutex;
use std::collections::HashSet;
use std::path::PathBuf;

/// The sessions' tables as a set of (cid, key): what `resolve` asks and fills.
#[derive(Default)]
struct Tables(Mutex<HashSet<(u64, PulledKey)>>);

impl ReceivedTables for Tables {
    fn holds(&self, cid: u64, key: &PulledKey) -> bool {
        self.0.lock().contains(&(cid, key.clone()))
    }
    fn load(&self, cid: u64, key: &PulledKey, _identity: FileIdentity) -> Result<(), String> {
        self.0.lock().insert((cid, key.clone()));
        Ok(())
    }
}

fn key(peer: u64, path: &str) -> PulledKey {
    PulledKey {
        peer_cid: Some(peer),
        virtual_directory: PathBuf::from(path),
    }
}

#[tokio::test]
async fn after_a_restart_a_received_file_is_readable_from_the_account_store() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("photo.png");
    std::fs::write(&file, b"png").unwrap();
    let store = MemoryKv::default();
    received_files::record(&store, 9, &file, identify(&file).unwrap(), 1, &|_| true)
        .await
        .unwrap();
    // A fresh process: the session's table is empty.
    let tables = Tables::default();
    let k = key(9, file.to_str().unwrap());
    assert_eq!(resolve(&tables, &store, 7, &k).await, Ok(true));
    assert!(tables.holds(7, &k), "loaded into the session's table");
}

#[tokio::test]
async fn a_file_never_received_or_from_someone_else_is_not_resolved() {
    let store = MemoryKv::default();
    let tables = Tables::default();
    assert_eq!(
        resolve(&tables, &store, 7, &key(9, "/t/9/x.png")).await,
        Ok(false)
    );
    let unnamed = PulledKey {
        peer_cid: None,
        virtual_directory: PathBuf::from("/t/9/x.png"),
    };
    assert_eq!(resolve(&tables, &store, 7, &unnamed).await, Ok(false));
}

#[test]
fn no_window_may_name_a_received_record() {
    assert!(crate::kernel::requests::local_db::reserved_key("received:9:/t/9/x.png").is_some());
}
