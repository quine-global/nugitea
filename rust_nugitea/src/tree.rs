//! Read-only git plumbing for the file-browser UI: listing a tree's
//! entries, reading a blob's content, and listing branches. Storage-tier
//! only — the app tier only ever sees the JSON these produce, via
//! `storage_server`'s HTTP endpoints and `storage_client`'s parsing of
//! them.

use std::path::Path;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::gitcmd;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    Tree,
    Blob,
}

/// One entry in a directory listing. This is the wire format produced by
/// `storage_server` and parsed by `storage_client` — shared directly
/// rather than duplicated on each side, the same way `mirror::Entry`
/// already crosses the app/storage boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeEntry {
    pub name: String,
    pub kind: EntryKind,
    pub sha: String,
}

/// Wire format for a blob read, likewise shared between `storage_server`
/// and `storage_client`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobContent {
    pub content: String,
    pub binary: bool,
}

/// Rejects a git ref or path starting with `-`. Subprocess args aren't
/// shell-interpreted, so there's no shell-injection risk here, but git
/// itself would still parse a leading `-` as an option rather than a
/// revision/path.
fn check_no_leading_dash(s: &str, what: &str) -> Result<()> {
    if s.starts_with('-') {
        bail!("invalid {what} {s:?}");
    }
    Ok(())
}

/// Lists the entries of the tree at `git_ref:path` (or `git_ref`'s root
/// tree, when path is empty) — one level, not recursive.
pub async fn list_tree(repo_path: &Path, git_ref: &str, path: &str) -> Result<Vec<TreeEntry>> {
    check_no_leading_dash(git_ref, "ref")?;
    check_no_leading_dash(path, "path")?;

    let treeish = if path.is_empty() {
        git_ref.to_string()
    } else {
        format!("{git_ref}:{path}")
    };
    let output = gitcmd::run_captured(repo_path, &["ls-tree", &treeish]).await?;

    let mut entries = Vec::new();
    for line in String::from_utf8_lossy(&output).lines() {
        let Some((meta, name)) = line.split_once('\t') else {
            continue;
        };
        let mut fields = meta.split(' ');
        let (Some(_mode), Some(kind), Some(sha)) = (fields.next(), fields.next(), fields.next()) else {
            continue;
        };
        let kind = match kind {
            "tree" => EntryKind::Tree,
            "blob" => EntryKind::Blob,
            _ => continue, // commit (submodule) entries etc. — not browsable here
        };
        entries.push(TreeEntry {
            name: name.to_string(),
            kind,
            sha: sha.to_string(),
        });
    }
    Ok(entries)
}

/// Reads the raw content of the blob at `git_ref:path`.
pub async fn read_blob(repo_path: &Path, git_ref: &str, path: &str) -> Result<Vec<u8>> {
    check_no_leading_dash(git_ref, "ref")?;
    check_no_leading_dash(path, "path")?;
    if path.is_empty() {
        bail!("empty path");
    }
    gitcmd::run_captured(repo_path, &["show", &format!("{git_ref}:{path}")]).await
}

/// Lists every blob path in `git_ref`'s tree, recursively — the full file
/// list the search island filters against.
pub async fn list_all_files(repo_path: &Path, git_ref: &str) -> Result<Vec<String>> {
    check_no_leading_dash(git_ref, "ref")?;
    let output = gitcmd::run_captured(repo_path, &["ls-tree", "-r", "--name-only", git_ref]).await?;
    Ok(String::from_utf8_lossy(&output).lines().map(str::to_string).collect())
}

/// Lists local branch names.
pub async fn list_branches(repo_path: &Path) -> Result<Vec<String>> {
    let output = gitcmd::run_captured(repo_path, &["for-each-ref", "--format=%(refname:short)", "refs/heads"]).await?;
    Ok(String::from_utf8_lossy(&output).lines().map(str::to_string).collect())
}
