//! Wraps invocation of the `git` subprocess, mirroring the pattern used by
//! the sibling Go implementation (and Gitea itself, in modules/git/gitcmd):
//! a small builder around a process-spawning API with a fixed baseline
//! environment, used both for one-shot commands and for long-lived piped
//! subprocesses wired into a network transport.

use std::path::Path;
use std::process::Stdio;

use anyhow::{bail, Result};
use tokio::process::{Child, Command};

/// baseline environment variables always set for git subprocesses, on top
/// of whatever the process already inherited, to keep behavior deterministic
/// and free of surprise user/system git config interference.
fn command(args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.args(args);
    cmd.env("GIT_CONFIG_NOSYSTEM", "1");
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env("LC_ALL", "C");
    cmd
}

/// Runs a git command to completion, discarding stdout, failing on a
/// non-zero exit status.
pub async fn run(dir: &Path, args: &[&str]) -> Result<()> {
    let output = command(args).current_dir(dir).output().await?;
    if !output.status.success() {
        bail!(
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// Runs a git command to completion and returns its stdout.
pub async fn run_captured(dir: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = command(args).current_dir(dir).output().await?;
    if !output.status.success() {
        bail!(
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(output.stdout)
}

/// Spawns a git command with stdin/stdout/stderr piped for interactive use
/// (the smart-HTTP and SSH transports stream into/out of these directly).
///
/// `dir` sets the working directory when present; when absent, callers are
/// expected to pass any repo path as a plain argument instead (used by the
/// SSH transport, mirroring the Go implementation's `cmd/serv.go`-style
/// invocation of `git upload-pack <path>` without a chdir).
pub fn spawn_piped(dir: Option<&Path>, args: &[&str], extra_env: &[(String, String)]) -> Result<Child> {
    let mut cmd = command(args);
    if let Some(dir) = dir {
        cmd.current_dir(dir);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in extra_env {
        cmd.env(key, value);
    }
    Ok(cmd.spawn()?)
}
