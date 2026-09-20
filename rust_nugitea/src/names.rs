//! Pure repo-name parsing and validation — no I/O, no network. Shared by
//! both the app tier (which only ever sees repo names, never local paths)
//! and the storage tier (which validates before touching disk), so the
//! "what makes a repo name safe" rule exists exactly once.

use anyhow::{bail, Result};

/// Validates that name is safe to use as a repo name: the only defense
/// against path traversal, since there is no auth layer to rely on.
pub fn validate(name: &str) -> Result<()> {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() || c == '_' => {}
        _ => bail!("invalid repo name {name:?}"),
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        bail!("invalid repo name {name:?}");
    }
    Ok(())
}

/// Strips a wire-form prefix/suffix — a leading '/' (SSH command paths) and
/// trailing ".git" (both transports) — and validates what's left. Turns
/// untrusted client-supplied input into a safe repo name, or None if it
/// isn't one.
pub fn from_wire_form(raw: &str) -> Option<String> {
    let raw = raw.strip_prefix('/').unwrap_or(raw);
    let name = raw.strip_suffix(".git")?;
    validate(name).ok()?;
    Some(name.to_string())
}
