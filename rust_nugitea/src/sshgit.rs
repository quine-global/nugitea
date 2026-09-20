//! Implements the SSH transport for git clone/push, mirroring the approach
//! in Gitea's modules/ssh/ssh.go + cmd/serv.go: read the raw SSH command,
//! validate the verb, and exec the matching git subcommand with stdio wired
//! directly to the session. Unlike Gitea, nugitea execs git directly from
//! the session handler instead of re-invoking its own binary, since there
//! is no permission database to hop into.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use bytes::Bytes;
use russh::keys::{Algorithm, PrivateKey};
use russh::server::{self, Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, Mutex};

use crate::auth::Authorizer;
use crate::gitcmd;
use crate::repo::Store;

struct Inner {
    repos: Arc<Store>,
    auth: Arc<dyn Authorizer>,
}

/// Handler is cloned fresh per incoming connection by `new_client`, so
/// `stdins` (channel -> subprocess stdin forwarder) is never shared across
/// connections even though channel IDs are reused per-connection.
#[derive(Clone)]
struct GitHandler {
    inner: Arc<Inner>,
    stdins: Arc<Mutex<HashMap<ChannelId, mpsc::UnboundedSender<Bytes>>>>,
}

impl server::Server for GitHandler {
    type Handler = Self;

    fn new_client(&mut self, _addr: Option<SocketAddr>) -> Self {
        GitHandler {
            inner: self.inner.clone(),
            stdins: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl server::Handler for GitHandler {
    type Error = russh::Error;

    // No users/login: accept any offered key. This is a stub seam,
    // structured like auth::Authorizer, for real key-checking later.
    async fn auth_publickey(
        &mut self,
        _user: &str,
        _key: &russh::keys::PublicKey,
    ) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let cmd_line = String::from_utf8_lossy(data).into_owned();
        match self.start(channel, &cmd_line, session.handle()).await {
            Ok(()) => {
                session.channel_success(channel)?;
            }
            Err(msg) => {
                session.data(channel, Bytes::from(format!("nugitea: {msg}\n")))?;
                session.channel_failure(channel)?;
                session.exit_status_request(channel, 1)?;
                session.close(channel)?;
            }
        }
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Some(tx) = self.stdins.lock().await.get(&channel) {
            let _ = tx.send(Bytes::copy_from_slice(data));
        }
        Ok(())
    }

    async fn channel_eof(
        &mut self,
        channel: ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        // Dropping the sender closes the forwarder's mpsc receiver, which
        // then drops the subprocess's stdin handle, delivering EOF to git.
        self.stdins.lock().await.remove(&channel);
        Ok(())
    }
}

impl GitHandler {
    async fn start(
        &self,
        channel: ChannelId,
        cmd_line: &str,
        handle: server::Handle,
    ) -> std::result::Result<(), String> {
        let (verb, repo_name) = parse_git_command(cmd_line)?;
        Store::validate_name(&repo_name).map_err(|e| e.to_string())?;
        if !self.inner.repos.exists(&repo_name) {
            return Err(format!("repository {repo_name:?} not found"));
        }
        let (allowed, git_arg) = match verb.as_str() {
            "git-upload-pack" => (self.inner.auth.allow_pull(&repo_name), "upload-pack"),
            "git-receive-pack" => (self.inner.auth.allow_push(&repo_name), "receive-pack"),
            _ => return Err(format!("unsupported command {verb:?}")),
        };
        if !allowed {
            return Err("forbidden".to_string());
        }
        let repo_path = self.inner.repos.path(&repo_name).map_err(|e| e.to_string())?;
        let repo_path_str = repo_path.to_string_lossy().to_string();

        let mut child = gitcmd::spawn_piped(None, &[git_arg, &repo_path_str], &[])
            .map_err(|e| e.to_string())?;

        let mut stdin = child.stdin.take().expect("piped stdin");
        let mut stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");

        let (tx, mut rx) = mpsc::unbounded_channel::<Bytes>();
        self.stdins.lock().await.insert(channel, tx);
        let stdins = self.stdins.clone();

        tokio::spawn(async move {
            let stdin_task = tokio::spawn(async move {
                while let Some(chunk) = rx.recv().await {
                    if stdin.write_all(&chunk).await.is_err() {
                        break;
                    }
                }
            });

            let out_handle = handle.clone();
            let stdout_task = tokio::spawn(async move {
                let mut buf = [0u8; 32 * 1024];
                loop {
                    match stdout.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if out_handle
                                .data(channel, Bytes::copy_from_slice(&buf[..n]))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                }
            });

            let err_handle = handle.clone();
            let stderr_task = tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                loop {
                    match stderr.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let _ = err_handle
                                .extended_data(channel, 1, Bytes::copy_from_slice(&buf[..n]))
                                .await;
                        }
                    }
                }
            });

            let status = child.wait().await;
            let _ = stdout_task.await;
            let _ = stderr_task.await;
            stdin_task.abort();
            stdins.lock().await.remove(&channel);

            let code = status.ok().and_then(|s| s.code()).unwrap_or(1) as u32;
            let _ = handle.exit_status_request(channel, code).await;
            let _ = handle.eof(channel).await;
            let _ = handle.close(channel).await;
        });

        Ok(())
    }
}

/// Starts the SSH server and blocks.
pub async fn serve(
    repos: Arc<Store>,
    auth: Arc<dyn Authorizer>,
    addr: SocketAddr,
    host_key_path: PathBuf,
) -> Result<()> {
    let key = load_or_create_host_key(&host_key_path)?;
    let config = Arc::new(server::Config {
        keys: vec![key],
        ..Default::default()
    });

    let mut handler = GitHandler {
        inner: Arc::new(Inner { repos, auth }),
        stdins: Arc::new(Mutex::new(HashMap::new())),
    };

    println!("ssh: listening on {addr}");
    handler.run_on_address(config, addr).await?;
    Ok(())
}

fn load_or_create_host_key(path: &Path) -> Result<PrivateKey> {
    if let Ok(data) = std::fs::read(path) {
        return PrivateKey::from_openssh(&data).context("parse ssh host key");
    }

    let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)
        .context("generate ssh host key")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let pem = key.to_openssh(russh::keys::ssh_key::LineEnding::LF)?;
    std::fs::write(path, pem.as_bytes())?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }

    Ok(key)
}

const ALLOWED_VERBS: [&str; 2] = ["git-upload-pack", "git-receive-pack"];

/// Extracts the verb and repo name from a raw SSH command line such as
/// `git-upload-pack '/demo.git'`.
fn parse_git_command(cmd_line: &str) -> std::result::Result<(String, String), String> {
    let fields = shell_split(cmd_line)?;
    if fields.len() != 2 {
        return Err("unsupported command".to_string());
    }
    let verb = fields[0].clone();
    if !ALLOWED_VERBS.contains(&verb.as_str()) {
        return Err(format!("unsupported command {verb:?}"));
    }
    let path = fields[1].as_str();
    let path = path.strip_prefix('/').unwrap_or(path);
    let path = path.strip_suffix(".git").unwrap_or(path);
    Ok((verb, path.to_string()))
}

/// Minimal POSIX-ish word splitting for the raw SSH command git clients
/// send, just enough to strip a single layer of matching quotes around the
/// repo path.
fn shell_split(s: &str) -> std::result::Result<Vec<String>, String> {
    let mut fields = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut in_field = false;

    for c in s.chars() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                cur.push(c);
            }
            continue;
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                in_field = true;
            }
            ' ' | '\t' => {
                if in_field {
                    fields.push(std::mem::take(&mut cur));
                    in_field = false;
                }
            }
            _ => {
                cur.push(c);
                in_field = true;
            }
        }
    }
    if quote.is_some() {
        return Err("unterminated quote".to_string());
    }
    if in_field {
        fields.push(cur);
    }
    Ok(fields)
}
