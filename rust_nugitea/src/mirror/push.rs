use anyhow::Result;

use crate::storage_client::StorageClient;

use super::store::Entry;
use super::REMOTE_NAME;

/// Runs the actual mirror push, equivalent to Gitea's `git push --mirror -f
/// <remote>` in services/mirror/mirror_push.go.
pub async fn sync_push(storage: &StorageClient, entry: &Entry) -> Result<()> {
    storage.run_git(&entry.repo, &["push", "--mirror", "-f", REMOTE_NAME]).await
}
