//! The app tier's SSH transport for git clone/push. Like `httpgit`, this is
//! a thin proxy: it validates the repo name, checks `Authorizer`, and
//! forwards the SSH session's stdin/stdout to the storage tier's raw
//! git-exec TCP relay (`git_exec_tcp`) instead of execing git locally.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use bytes::Bytes;
use russh::keys::{Algorithm, PrivateKey};
use russh::server::{self, Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, Mutex};

use crate::auth::Authorizer;
use crate::service::Service;
use crate::storage_client::StorageClient;

struct Inner {
    storage: Arc<StorageClient>,
    auth: Arc<dyn Authorizer>,
}

/// Handler is cloned fresh per incoming connection by `new_client`, so
/// `stdins` (channel -> proxy-request-body forwarder) is never shared
/// across connections even though channel IDs are reused per-connection.
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
        // Dropping the sender closes the forwarder stream, which signals
        // the end of the request body to the storage tier.
        self.stdins.lock().await.remove(&channel);
        Ok(())
    }
}

impl GitHandler {
    async fn start(&self, channel: ChannelId, cmd_line: &str, handle: server::Handle) -> Result<()> {
        let fields = shlex::split(cmd_line).context("unsupported command")?;
        let [verb, repo_ref] = fields.as_slice() else {
            bail!("unsupported command");
        };
        let service = Service::from_ssh_verb(verb).with_context(|| format!("unsupported command {verb:?}"))?;
        let repo_name = self
            .inner
            .storage
            .resolve(repo_ref)
            .await
            .with_context(|| format!("repository {repo_ref:?} not found"))?;
        if !service.allow(self.inner.auth.as_ref(), &repo_name) {
            bail!("forbidden");
        }

        // git's SSH-driven protocol is interactive (the server writes a
        // ref advertisement before reading any stdin, both directions stay
        // live at once) — that doesn't fit HTTP/1.1 request/response, so
        // this goes over a raw TCP relay instead of the HTTP API. See
        // git_exec_tcp for why.
        let stream = self.inner.storage.connect_git_exec(&repo_name, service).await?;
        let (mut read_half, write_half) = stream.into_split();

        let (tx, mut rx) = mpsc::unbounded_channel::<Bytes>();
        self.stdins.lock().await.insert(channel, tx);
        let stdins = self.stdins.clone();

        tokio::spawn(async move {
            let write_task = tokio::spawn(async move {
                let mut write_half = write_half;
                while let Some(chunk) = rx.recv().await {
                    if write_half.write_all(&chunk).await.is_err() {
                        break;
                    }
                }
                // Half-close so the storage tier sees EOF on its read side
                // once the SSH client is done sending, and can close the
                // subprocess's stdin in turn.
                let _ = write_half.shutdown().await;
            });

            let mut buf = [0u8; 32 * 1024];
            let mut ok = true;
            loop {
                match read_half.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        if handle.data(channel, Bytes::copy_from_slice(&buf[..n])).await.is_err() {
                            ok = false;
                            break;
                        }
                    }
                    Err(_) => {
                        ok = false;
                        break;
                    }
                }
            }
            write_task.abort();
            stdins.lock().await.remove(&channel);

            // The exact git subprocess exit code doesn't cross the proxy
            // hop; this reports whether the relay itself succeeded,
            // matching how the HTTP transport has never surfaced git's
            // literal exit code to its client either — it only logs
            // failures server-side.
            let _ = handle.exit_status_request(channel, if ok { 0 } else { 1 }).await;
            let _ = handle.eof(channel).await;
            let _ = handle.close(channel).await;
        });

        Ok(())
    }
}

/// Starts the SSH server and blocks.
pub async fn serve(storage: Arc<StorageClient>, auth: Arc<dyn Authorizer>, addr: SocketAddr, host_key_path: PathBuf) -> Result<()> {
    let key = load_or_create_host_key(&host_key_path)?;
    let config = Arc::new(server::Config {
        keys: vec![key],
        ..Default::default()
    });

    let mut handler = GitHandler {
        inner: Arc::new(Inner { storage, auth }),
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
