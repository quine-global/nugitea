use anyhow::Result;

use crate::gitcmd;
use crate::repo::Store as RepoStore;

use super::store::Entry;
use super::REMOTE_NAME;

/// Runs the actual mirror push, equivalent to Gitea's `git push --mirror -f
/// <remote>` in services/mirror/mirror_push.go.
pub async fn sync_push(repos: &RepoStore, entry: &Entry) -> Result<()> {
    let repo_path = repos.path(&entry.repo)?;
    gitcmd::run(&repo_path, &["push", "--mirror", "-f", REMOTE_NAME]).await
}
