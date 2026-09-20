use anyhow::Result;

use crate::gitcmd;
use crate::repo::Store as RepoStore;

use super::store::Entry;
use super::REMOTE_NAME;

/// Runs the actual fetch for a configured pull mirror, equivalent to
/// Gitea's `git fetch --prune --tags <remote>` in
/// services/mirror/mirror_pull.go.
pub async fn sync_pull(repos: &RepoStore, entry: &Entry) -> Result<()> {
    let repo_path = repos.path(&entry.repo)?;
    gitcmd::run(&repo_path, &["fetch", "--prune", "--tags", REMOTE_NAME]).await
}
