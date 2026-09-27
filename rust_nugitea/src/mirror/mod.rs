//! Implements pull/push mirroring: periodically running `git fetch --prune
//! --tags <remote>` (pull) or `git push --mirror -f <remote>` (push)
//! against a remote configured via `git remote add --mirror=fetch|push`,
//! mirroring services/mirror/mirror_{pull,push}.go. Runs on the app tier;
//! the actual git commands are proxied to the storage tier over
//! `StorageClient`, same as httpgit/sshgit.

mod pull;
mod push;
mod scheduler;
mod store;

pub use scheduler::Scheduler;
pub use store::{Direction, Entry, Store};

use std::time::Duration;

use anyhow::{bail, Result};

use crate::storage_client::StorageClient;

/// The git remote name nugitea uses for every configured mirror; each repo
/// has at most one mirror per direction.
const REMOTE_NAME: &str = "mirror";

/// Points repo_name's `mirror` remote at remote_url and registers the sync
/// schedule, mirroring Gitea's pull/push-mirror setup (`git remote add
/// --mirror=fetch|push`). `add_pull`/`add_push` are thin wrappers over this
/// shared setup, differing only in which `--mirror=` flag and Direction
/// apply.
async fn configure(
    storage: &StorageClient,
    mirrors: &Store,
    repo_name: &str,
    remote_url: &str,
    interval: Duration,
    direction: Direction,
) -> Result<()> {
    if !storage.exists(repo_name).await {
        bail!("repo {repo_name:?} does not exist");
    }

    let mirror_flag = match direction {
        Direction::Pull => "--mirror=fetch",
        Direction::Push => "--mirror=push",
    };

    // Ignore the error: fine if no remote existed yet.
    let _ = storage.run_git(repo_name, &["remote", "remove", REMOTE_NAME]).await;

    storage.run_git(repo_name, &["remote", "add", mirror_flag, REMOTE_NAME, remote_url]).await?;

    mirrors
        .add(Entry {
            repo: repo_name.to_string(),
            remote_url: remote_url.to_string(),
            direction,
            interval_seconds: interval.as_secs(),
            last_sync_unix: 0,
        })
        .await
}

pub async fn add_pull(
    storage: &StorageClient,
    mirrors: &Store,
    repo_name: &str,
    remote_url: &str,
    interval: Duration,
) -> Result<()> {
    configure(storage, mirrors, repo_name, remote_url, interval, Direction::Pull).await
}

pub async fn add_push(
    storage: &StorageClient,
    mirrors: &Store,
    repo_name: &str,
    remote_url: &str,
    interval: Duration,
) -> Result<()> {
    configure(storage, mirrors, repo_name, remote_url, interval, Direction::Push).await
}
