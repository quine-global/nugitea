//! Implements pull/push mirroring: periodically running `git fetch --prune
//! --tags <remote>` (pull) or `git push --mirror -f <remote>` (push)
//! against a remote configured via `git remote add --mirror=fetch|push`,
//! mirroring services/mirror/mirror_{pull,push}.go.

mod pull;
mod push;
mod scheduler;
mod store;

pub use pull::add_pull;
pub use push::add_push;
pub use scheduler::Scheduler;
pub use store::Store;

/// The git remote name nugitea uses for every configured mirror; each repo
/// has at most one mirror per direction.
const REMOTE_NAME: &str = "mirror";
