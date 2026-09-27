//! Pure repo-path parsing and validation — no I/O, no network. Shared by
//! both the app tier (which only ever sees repo paths, never local paths)
//! and the storage tier (which validates before touching disk), so the
//! "what makes a repo path safe" rule exists exactly once.
//!
//! A repo path is `owner/name`, or `org/sub-org/.../name` for repos under
//! nested orgs (see `accounts`): one or more owner segments, then the repo
//! name, each segment validated on its own.

use anyhow::{bail, Result};

/// The most owner segments a repo path can have — GitLab's subgroup
/// depth limit.
pub const MAX_OWNER_DEPTH: usize = 20;

/// Validates that name is safe to use as a single path segment (an
/// account slug or repo name): the only defense against path traversal
/// on the storage tier, which trusts its caller.
pub fn validate(name: &str) -> Result<()> {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() || c == '_' => {}
        _ => bail!("invalid repo name {name:?}"),
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')) {
        bail!("invalid repo name {name:?}");
    }
    Ok(())
}

/// Validates a full `owner[/sub-org...]/name` repo path. Owner segments
/// can't end in `.git`, so a bare repo's directory on disk is never
/// mistaken for an owner's, or vice versa.
pub fn validate_repo_path(path: &str) -> Result<()> {
    let segs: Vec<&str> = path.split('/').collect();
    if segs.len() < 2 || segs.len() > MAX_OWNER_DEPTH + 1 {
        bail!("invalid repo name {path:?}: expected owner/name");
    }
    for seg in &segs {
        validate(seg).map_err(|_| anyhow::anyhow!("invalid repo name {path:?}"))?;
    }
    if segs[..segs.len() - 1].iter().any(|s| s.to_ascii_lowercase().ends_with(".git")) {
        bail!("invalid repo name {path:?}");
    }
    Ok(())
}

/// Strips a wire-form prefix/suffix — a leading '/' (SSH command paths) and
/// trailing ".git" (both transports) — and validates what's left. Turns
/// untrusted client-supplied input into a safe repo path, or None if it
/// isn't one.
pub fn from_wire_form(raw: &str) -> Option<String> {
    let raw = raw.strip_prefix('/').unwrap_or(raw);
    let path = raw.strip_suffix(".git")?;
    validate_repo_path(path).ok()?;
    Some(path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_paths() {
        assert!(validate_repo_path("alice/dotfiles").is_ok());
        assert!(validate_repo_path("acme/platform/api").is_ok());
        assert!(validate_repo_path("flat").is_err());
        assert!(validate_repo_path("acme//api").is_err());
        assert!(validate_repo_path("acme/../api").is_err());
        assert!(validate_repo_path("/acme/api").is_err());
        assert!(validate_repo_path("x.git/api").is_err());
    }

    #[test]
    fn wire_forms() {
        assert_eq!(from_wire_form("/acme/platform/api.git").as_deref(), Some("acme/platform/api"));
        assert_eq!(from_wire_form("acme/api.git").as_deref(), Some("acme/api"));
        assert_eq!(from_wire_form("acme/api"), None);
        assert_eq!(from_wire_form("api.git"), None);
    }
}
