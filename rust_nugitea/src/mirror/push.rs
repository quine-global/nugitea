use std::time::Duration;

use anyhow::{bail, Result};

use crate::gitcmd;
use crate::repo::Store as RepoStore;

use super::store::{Direction, Entry, Store as MirrorStore};
use super::REMOTE_NAME;

/// Configures repo_name to periodically mirror-push to remote_url,
/// mirroring Gitea's push-mirror setup: `git remote add --mirror=push`.
pub async fn add_push(
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
        &["remote", "add", "--mirror=push", REMOTE_NAME, remote_url],
    )
    .await?;

    mirrors
        .add(Entry {
            repo: repo_name.to_string(),
            remote_url: remote_url.to_string(),
            direction: Direction::Push,
            interval_seconds: interval.as_secs(),
            last_sync_unix: 0,
        })
        .await
}

/// Runs the actual mirror push, equivalent to Gitea's `git push --mirror -f
/// <remote>` in services/mirror/mirror_push.go.
pub async fn sync_push(repos: &RepoStore, entry: &Entry) -> Result<()> {
    let repo_path = repos.path(&entry.repo)?;
    gitcmd::run(&repo_path, &["push", "--mirror", "-f", REMOTE_NAME]).await
}
