use std::time::Duration;

use anyhow::{bail, Result};

use crate::gitcmd;
use crate::repo::Store as RepoStore;

use super::store::{Direction, Entry, Store as MirrorStore};
use super::REMOTE_NAME;

/// Configures repo_name to periodically fetch from remote_url, mirroring
/// Gitea's pull-mirror setup: `git remote add --mirror=fetch`.
pub async fn add_pull(
    repos: &RepoStore,
    mirrors: &MirrorStore,
    repo_name: &str,
    remote_url: &str,
    interval: Duration,
) -> Result<()> {
    let repo_path = repos.path(repo_name)?;
    if !repos.exists(repo_name) {
        bail!("repo {repo_name:?} does not exist");
    }

    // Ignore the error: fine if no remote existed yet.
    let _ = gitcmd::run(&repo_path, &["remote", "remove", REMOTE_NAME]).await;

    gitcmd::run(
        &repo_path,
        &["remote", "add", "--mirror=fetch", REMOTE_NAME, remote_url],
    )
    .await?;

    mirrors
        .add(Entry {
            repo: repo_name.to_string(),
            remote_url: remote_url.to_string(),
            direction: Direction::Pull,
            interval_seconds: interval.as_secs(),
            last_sync_unix: 0,
        })
        .await
}

/// Runs the actual fetch for a configured pull mirror, equivalent to
/// Gitea's `git fetch --prune --tags <remote>` in
/// services/mirror/mirror_pull.go.
pub async fn sync_pull(repos: &RepoStore, entry: &Entry) -> Result<()> {
    let repo_path = repos.path(&entry.repo)?;
    gitcmd::run(&repo_path, &["fetch", "--prune", "--tags", REMOTE_NAME]).await
}
