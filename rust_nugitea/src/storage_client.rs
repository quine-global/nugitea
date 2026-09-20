//! The app tier's client for the storage tier's internal HTTP API (see
//! `storage_server`). The app tier holds one of these instead of a local
//! `storage::Store` — it never sees a local git path, only repo names.

use anyhow::{bail, Result};
use bytes::Bytes;
use futures::Stream;
use serde::Serialize;
use tokio::net::TcpStream;

use crate::names;
use crate::service::Service;
use crate::tree::{BlobContent, TreeEntry};

#[derive(Clone)]
pub struct StorageClient {
    base_url: String,
    tcp_addr: String,
    http: reqwest::Client,
}

impl StorageClient {
    pub fn new(base_url: String, tcp_addr: String) -> Self {
        StorageClient {
            base_url: base_url.trim_end_matches('/').to_string(),
            tcp_addr,
            http: reqwest::Client::new(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    /// Turns a wire-form repo reference (HTTP path segment or SSH command
    /// path) into a validated, existing repo name — the app tier's
    /// equivalent of the old local `storage::Store::resolve`.
    pub async fn resolve(&self, raw: &str) -> Option<String> {
        let name = names::from_wire_form(raw)?;
        if self.exists(&name).await {
            Some(name)
        } else {
            None
        }
    }

    pub async fn exists(&self, name: &str) -> bool {
        self.http
            .head(self.url(&format!("/repos/{name}")))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }

    pub async fn init_bare(&self, name: &str) -> Result<()> {
        let resp = self.http.post(self.url(&format!("/repos/{name}"))).send().await?;
        if !resp.status().is_success() {
            bail!("create repo {name:?}: {}", resp.text().await.unwrap_or_default());
        }
        Ok(())
    }

    pub async fn list(&self) -> Result<Vec<String>> {
        Ok(self.http.get(self.url("/repos")).send().await?.error_for_status()?.json().await?)
    }

    /// Runs a non-streaming git command against a repo (remote add/remove,
    /// fetch, push --mirror) — used by the mirror scheduler.
    pub async fn run_git(&self, name: &str, args: &[&str]) -> Result<()> {
        #[derive(Serialize)]
        struct Req<'a> {
            args: &'a [&'a str],
        }
        let resp = self
            .http
            .post(self.url(&format!("/repos/{name}/git")))
            .json(&Req { args })
            .send()
            .await?;
        if !resp.status().is_success() {
            bail!("git {args:?} on {name:?}: {}", resp.text().await.unwrap_or_default());
        }
        Ok(())
    }

    /// Fetches the pkt-line-framed ref advertisement for `GET .../info/refs`.
    pub async fn info_refs(&self, name: &str, service: Service) -> Result<Vec<u8>> {
        let resp = self
            .http
            .get(self.url(&format!("/repos/{name}/info-refs")))
            .query(&[("service", format!("git-{}", service.git_arg()))])
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.bytes().await?.to_vec())
    }

    /// Proxies a streamed git-upload-pack/git-receive-pack request over
    /// the HTTP API's `--stateless-rpc` endpoints: `body` is forwarded as
    /// the request body, the response body is returned as a byte stream to
    /// forward onward as an HTTP response. Used by the HTTP transport only
    /// — see `connect_git_exec` for the SSH transport's interactive case.
    pub async fn stream_git(
        &self,
        name: &str,
        service: Service,
        git_protocol_header: Option<&str>,
        content_encoding_gzip: bool,
        body: reqwest::Body,
    ) -> Result<impl Stream<Item = reqwest::Result<Bytes>>> {
        let mut req = self.http.post(self.url(&format!("/repos/{name}/{}", service.git_arg()))).body(body);
        if let Some(proto) = git_protocol_header {
            req = req.header("Git-Protocol", proto);
        }
        if content_encoding_gzip {
            req = req.header(reqwest::header::CONTENT_ENCODING, "gzip");
        }
        let resp = req.send().await?.error_for_status()?;
        Ok(resp.bytes_stream())
    }

    pub async fn branches(&self, name: &str) -> Result<Vec<String>> {
        Ok(self
            .http
            .get(self.url(&format!("/repos/{name}/branches")))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    /// Lists the entries of the tree at `git_ref:path` (or `git_ref`'s
    /// root tree, when path is empty) for the file-browser UI.
    pub async fn tree(&self, name: &str, git_ref: &str, path: &str) -> Result<Vec<TreeEntry>> {
        let url = if path.is_empty() {
            self.url(&format!("/repos/{name}/tree/{git_ref}"))
        } else {
            self.url(&format!("/repos/{name}/tree/{git_ref}/{path}"))
        };
        Ok(self.http.get(url).send().await?.error_for_status()?.json().await?)
    }

    /// Reads a blob's content for the file-browser UI.
    pub async fn blob(&self, name: &str, git_ref: &str, path: &str) -> Result<BlobContent> {
        let url = self.url(&format!("/repos/{name}/blob/{git_ref}/{path}"));
        Ok(self.http.get(url).send().await?.error_for_status()?.json().await?)
    }

    /// Opens a raw TCP connection to the storage tier's git-exec relay
    /// (`git_exec_tcp`) for the SSH transport's interactive git protocol,
    /// which needs true simultaneous bidirectional streaming — not a fit
    /// for HTTP/1.1 request/response framing. See `git_exec_tcp` for why.
    pub async fn connect_git_exec(&self, name: &str, service: Service) -> Result<TcpStream> {
        use tokio::io::AsyncWriteExt;
        let mut stream = TcpStream::connect(&self.tcp_addr).await?;
        stream.write_all(format!("{} {name}\n", service.git_arg()).as_bytes()).await?;
        Ok(stream)
    }
}
