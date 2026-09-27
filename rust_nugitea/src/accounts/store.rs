use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::sync::Mutex;

use super::Directory;

/// Persists the `Directory` as one JSON file under the app tier's state
/// dir, the same way `mirror::Store` persists mirrors.json. Every read
/// re-loads the file rather than caching it, so changes the CLI makes
/// while `serve` is running show up without a restart.
pub struct Store {
    path: PathBuf,
    lock: Mutex<()>,
}

impl Store {
    /// Returns a Store backed by accounts.json under root, creating root if
    /// it doesn't already exist.
    pub fn new(root: &Path) -> Result<Self> {
        std::fs::create_dir_all(root).with_context(|| format!("create state dir {root:?}"))?;
        Ok(Store { path: root.join("accounts.json"), lock: Mutex::new(()) })
    }

    async fn read(&self) -> Result<Directory> {
        match tokio::fs::read(&self.path).await {
            Ok(data) => Ok(serde_json::from_slice(&data).with_context(|| format!("parse {:?}", self.path))?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Directory::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// A snapshot of the current directory.
    pub async fn load(&self) -> Result<Directory> {
        let _guard = self.lock.lock().await;
        self.read().await
    }

    /// Applies f to the current directory and saves the result, unless f
    /// fails — `Directory` validates every change, so a failed f leaves
    /// the file untouched.
    pub async fn update<T>(&self, f: impl FnOnce(&mut Directory) -> Result<T>) -> Result<T> {
        let _guard = self.lock.lock().await;
        let mut dir = self.read().await?;
        let out = f(&mut dir)?;
        let data = serde_json::to_vec_pretty(&dir)?;
        let tmp = self.path.with_extension("json.tmp");
        tokio::fs::write(&tmp, data).await?;
        tokio::fs::rename(&tmp, &self.path).await?;
        Ok(out)
    }
}
