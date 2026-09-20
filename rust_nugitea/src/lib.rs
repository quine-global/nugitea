//! nugitea is a minimal git server split into two tiers, the way GitLab
//! splits the Rails app from Gitaly: an app tier (`bin/nugitea.rs`) that
//! speaks git's HTTP/SSH protocols and proxies the actual git work to a
//! storage tier (`bin/nugitea-storaged.rs`) that owns the repo-root disk.
//! No users, no auth beyond the `Authorizer` seam — just clone, push,
//! mirror, and a minimal read-only file-browser web UI (`webui`),
//! implemented by shelling out to git the same way Gitea does. See the
//! sibling Go implementation for the original single-process design this
//! was split from.

pub mod auth;
pub mod git_exec_tcp;
pub mod gitcmd;
pub mod httpgit;
pub mod mirror;
pub mod names;
pub mod service;
pub mod sshgit;
pub mod storage;
pub mod storage_client;
pub mod storage_server;
pub mod tree;
pub mod webui;
