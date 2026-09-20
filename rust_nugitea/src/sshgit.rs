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

use anyhow::{bail, Context, Result};
use bytes::Bytes;
use russh::keys::{Algorithm, PrivateKey};
use russh::server::{self, Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, Mutex};

use crate::auth::Authorizer;
use crate::gitcmd;
use crate::repo::Store;
use crate::service::Service;

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
            Err(e) => {
                session.data(channel, Bytes::from(format!("nugitea: {e}\n")))?;
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

/// Reads from `reader` until EOF or error, forwarding each chunk to the
/// channel as either normal data (`ext: None`) or extended data on the
/// given code (`ext: Some(code)`, e.g. `Some(1)` for stderr). Shared by the
/// stdout and stderr pumps in `GitHandler::start`.
async fn pump_to_channel(mut reader: impl AsyncRead + Unpin, channel: ChannelId, handle: server::Handle, ext: Option<u32>) {
    let mut buf = [0u8; 32 * 1024];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let chunk = Bytes::copy_from_slice(&buf[..n]);
                let sent = match ext {
                    None => handle.data(channel, chunk).await.map_err(|_| ()),
                    Some(code) => handle.extended_data(channel, code, chunk).await.map_err(|_| ()),
                };
                if sent.is_err() {
                    break;
                }
            }
        }
    }
}

impl GitHandler {
    async fn start(&self, channel: ChannelId, cmd_line: &str, handle: server::Handle) -> Result<()> {
        let fields = shlex::split(cmd_line).context("unsupported command")?;
        let [verb, repo_ref] = fields.as_slice() else {
            bail!("unsupported command");
        };
        let service = Service::from_ssh_verb(verb).with_context(|| format!("unsupported command {verb:?}"))?;
        let (repo_name, repo_path) = self
            .inner
            .repos
            .resolve(repo_ref)
            .await
            .with_context(|| format!("repository {repo_ref:?} not found"))?;
        if !service.allow(self.inner.auth.as_ref(), &repo_name) {
            bail!("forbidden");
        }
        let repo_path_str = repo_path.to_string_lossy().to_string();

        let mut child = gitcmd::spawn_piped(None, &[service.git_arg(), &repo_path_str], &[])?;

        let mut stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");

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

            let stdout_task = tokio::spawn(pump_to_channel(stdout, channel, handle.clone(), None));
            let stderr_task = tokio::spawn(pump_to_channel(stderr, channel, handle.clone(), Some(1)));

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
pub async fn serve(repos: Arc<Store>, auth: Arc<dyn Authorizer>, addr: SocketAddr, host_key_path: PathBuf) -> Result<()> {
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

    let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).context("generate ssh host key")?;
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
