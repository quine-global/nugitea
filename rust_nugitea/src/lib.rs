//! nugitea is a minimal git server split into two tiers, the way GitLab
//! splits the Rails app from Gitaly: an app tier (`bin/nugitea.rs`) that
//! speaks git's HTTP/SSH protocols and proxies the actual git work to a
//! storage tier (`bin/nugitea-storaged.rs`) that owns the repo-root disk.
//! Repos are owned by users and (optionally nested) orgs (`accounts`), but
//! there's no login and no auth beyond the `Authorizer` seam — just clone,
//! push, mirror, and a GitHub-compatible GraphQL API (`graphql`) for
//! browsing owners and repo contents, implemented by shelling out to git
//! the same way Gitea does.
//! The actual browsing UI is a separate Nuxt.js app (`web/`) that queries
//! that API — see its README for why, and the sibling Go implementation
//! for the original single-process design this was split from.

pub mod accounts;
pub mod auth;
pub mod git_exec_tcp;
pub mod gitcmd;
pub mod graphql;
pub mod httpgit;
pub mod mirror;
pub mod names;
pub mod service;
pub mod sshgit;
pub mod storage;
pub mod storage_client;
pub mod storage_server;
pub mod tree;
