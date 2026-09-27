use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Pull,
    Push,
}

/// One configured mirror.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub repo: String,
    pub remote_url: String,
    pub direction: Direction,
    pub interval_seconds: u64,
    #[serde(default)]
    pub last_sync_unix: u64,
}

impl Entry {
    fn matches(&self, other: &Entry) -> bool {
        self.direction == other.direction && self.repo == other.repo
    }
}

pub fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

/// Persists mirror configuration to a JSON file under the repo root.
pub struct Store {
    path: PathBuf,
    lock: Mutex<()>,
}

impl Store {
    /// Returns a Store backed by mirrors.json under root, creating root if
    /// it doesn't already exist.
    pub fn new(root: &std::path::Path) -> Result<Self> {
        std::fs::create_dir_all(root).with_context(|| format!("create state dir {root:?}"))?;
        Ok(Store { path: root.join("mirrors.json"), lock: Mutex::new(()) })
    }

    async fn load(&self) -> Result<Vec<Entry>> {
        match tokio::fs::read(&self.path).await {
            Ok(data) => Ok(serde_json::from_slice(&data)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
            Err(e) => Err(e.into()),
        }
    }

    async fn save(&self, entries: &[Entry]) -> Result<()> {
        let data = serde_json::to_vec_pretty(entries)?;
        let tmp = self.path.with_extension("json.tmp");
        tokio::fs::write(&tmp, data).await?;
        tokio::fs::rename(&tmp, &self.path).await?;
        Ok(())
    }

    /// Registers a new mirror, replacing any existing one with the same
    /// repo+direction.
    pub async fn add(&self, entry: Entry) -> Result<()> {
        let _guard = self.lock.lock().await;
        let mut entries = self.load().await?;
        entries.retain(|e| !e.matches(&entry));
        entries.push(entry);
        self.save(&entries).await
    }

    /// Returns every configured mirror.
    pub async fn all(&self) -> Result<Vec<Entry>> {
        let _guard = self.lock.lock().await;
        self.load().await
    }

    /// Updates last_sync_unix for the mirror matching target.
    pub async fn touch(&self, target: &Entry, when_unix: u64) -> Result<()> {
        let _guard = self.lock.lock().await;
        let mut entries = self.load().await?;
        for e in entries.iter_mut() {
            if e.matches(target) {
                e.last_sync_unix = when_unix;
            }
        }
        self.save(&entries).await
    }
}
