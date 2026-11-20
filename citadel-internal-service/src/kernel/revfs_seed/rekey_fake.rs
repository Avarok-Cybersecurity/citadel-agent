//! A world for re-key passes to run against: objects by place and path, the local files pulls
//! write, the index, and a switch that stops every call from the n-th on (a store there tears
//! its object, as a store that dies half way does).

use super::*;
use crate::kernel::revfs_seed::index::{IndexEntry, Location, RevfsIndex};
use parking_lot::Mutex;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Stored {
    Sealed {
        key: KeyId,
        contents: Vec<u8>,
    },
    /// A store that died half way.
    Torn,
}

#[derive(Default)]
pub(super) struct World {
    pub objects: HashMap<(Location, String), Stored>,
    pub locals: HashMap<PathBuf, Vec<u8>>,
    pub index: RevfsIndex,
    pub calls: usize,
    /// The call that fails (a store there tears its object); every later call fails too.
    pub stop_at: Option<usize>,
    pub offline: Vec<Location>,
}

pub(super) struct Fake(pub Mutex<World>);

impl Fake {
    fn step(&self) -> Result<(), String> {
        let mut world = self.0.lock();
        world.calls += 1;
        match world.stop_at {
            Some(stop) if world.calls > stop => Err("stopped".into()),
            _ => Ok(()),
        }
    }
}

#[async_trait]
impl RekeyPorts for Fake {
    fn reachable(&self, location: Location) -> bool {
        !self.0.lock().offline.contains(&location)
    }
    async fn pull(
        &self,
        location: Location,
        path: &str,
        key: &RevfsKey,
    ) -> Result<PathBuf, String> {
        self.step()?;
        let mut world = self.0.lock();
        match world.objects.get(&(location, path.to_string())) {
            Some(Stored::Sealed {
                key: sealed,
                contents,
            }) if *sealed == key.id() => {
                let local = PathBuf::from(format!("/local/{}", world.locals.len()));
                let contents = contents.clone();
                world.locals.insert(local.clone(), contents);
                Ok(local)
            }
            _ => Err("does not open".into()),
        }
    }
    async fn push(
        &self,
        location: Location,
        path: &str,
        _level: u8,
        local: &Path,
        key: &RevfsKey,
    ) -> Result<(), String> {
        let at = (location, path.to_string());
        if let Err(stopped) = self.step() {
            self.0.lock().objects.insert(at, Stored::Torn);
            return Err(stopped);
        }
        let mut world = self.0.lock();
        let contents = world.locals.get(local).cloned().ok_or("no local file")?;
        world.objects.insert(
            at,
            Stored::Sealed {
                key: key.id(),
                contents,
            },
        );
        Ok(())
    }
    async fn delete(&self, location: Location, path: &str) -> Result<(), String> {
        self.step()?;
        self.0.lock().objects.remove(&(location, path.to_string()));
        Ok(())
    }
    async fn discard(&self, local: &Path) {
        self.0.lock().locals.remove(local);
    }
    async fn record(&self, entry: &IndexEntry) -> Result<(), String> {
        self.step()?;
        self.0.lock().index.upsert(entry.clone());
        Ok(())
    }
}
