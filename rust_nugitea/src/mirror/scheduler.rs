use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::sync::Mutex;

use crate::storage_client::StorageClient;

use super::pull::sync_pull;
use super::push::sync_push;
use super::store::{unix_now, Direction, Entry, Store as MirrorStore};

/// How often the scheduler checks for due mirrors.
const TICK_INTERVAL: Duration = Duration::from_secs(30);

/// Periodically syncs configured mirrors whose interval has elapsed.
pub struct Scheduler {
    storage: Arc<StorageClient>,
    mirrors: Arc<MirrorStore>,
    locks: StdMutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl Scheduler {
    pub fn new(storage: Arc<StorageClient>, mirrors: Arc<MirrorStore>) -> Arc<Self> {
        Arc::new(Scheduler {
            storage,
            mirrors,
            locks: StdMutex::new(HashMap::new()),
        })
    }

    /// Ticks forever.
    pub async fn run(self: Arc<Self>) {
        let mut ticker = tokio::time::interval(TICK_INTERVAL);
        loop {
            self.tick().await;
            ticker.tick().await;
        }
    }

    async fn tick(self: &Arc<Self>) {
        let entries = match self.mirrors.all().await {
            Ok(e) => e,
            Err(e) => {
                eprintln!("mirror scheduler: load mirrors: {e}");
                return;
            }
        };
        let now = unix_now();
        for entry in entries {
            let due = entry.last_sync_unix + entry.interval_seconds;
            if now < due {
                continue;
            }
            let this = self.clone();
            tokio::spawn(async move { this.sync(entry).await });
        }
    }

    async fn sync(self: Arc<Self>, entry: Entry) {
        let lock = self.lock_for(&entry);
        let Ok(_guard) = lock.try_lock() else {
            return; // a sync for this mirror is already running
        };

        let result = match entry.direction {
            Direction::Pull => sync_pull(&self.storage, &entry).await,
            Direction::Push => sync_push(&self.storage, &entry).await,
        };
        if let Err(e) = result {
            eprintln!("mirror sync {:?} {}: {e}", entry.direction, entry.repo);
        }
        if let Err(e) = self.mirrors.touch(&entry, unix_now()).await {
            eprintln!("mirror sync {:?} {}: record last sync: {e}", entry.direction, entry.repo);
        }
    }

    fn lock_for(&self, entry: &Entry) -> Arc<Mutex<()>> {
        let mut locks = self.locks.lock().unwrap();
        locks
            .entry(format!("{:?}:{}", entry.direction, entry.repo))
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
}
