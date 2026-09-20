use anyhow::Result;

use crate::storage_client::StorageClient;

use super::store::Entry;
use super::REMOTE_NAME;

/// Runs the actual fetch for a configured pull mirror, equivalent to
/// Gitea's `git fetch --prune --tags <remote>` in
/// services/mirror/mirror_pull.go.
pub async fn sync_pull(storage: &StorageClient, entry: &Entry) -> Result<()> {
    storage.run_git(&entry.repo, &["fetch", "--prune", "--tags", REMOTE_NAME]).await
}
