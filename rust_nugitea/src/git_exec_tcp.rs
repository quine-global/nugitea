//! Raw TCP duplex relay for the SSH transport's interactive git protocol.
//!
//! Unlike the HTTP API's upload-pack/receive-pack endpoints (which run git
//! with `--stateless-rpc`, a request-then-response protocol suited to a
//! single HTTP request/response), SSH git clients speak git's traditional
//! interactive protocol: the server writes a ref advertisement to stdout
//! *before* reading anything from stdin, and both directions stay live
//! simultaneously. HTTP/1.1 request/response framing can't reliably
//! support that duplex pattern, so this is a plain TCP socket instead —
//! the same shape as talking to a local subprocess's pipes, just across a
//! network hop. Trusts its caller (the app tier), same as the HTTP API.
//!
//! Wire protocol: connect, write one line `<verb> <repo-name>\n` (verb is
//! "upload-pack" or "receive-pack"), then the connection becomes a raw
//! byte relay to/from that git subprocess until it exits.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{bail, Result};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use crate::gitcmd;
use crate::storage::Store;

pub async fn serve(store: Arc<Store>, addr: SocketAddr) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;
    println!("storage: git-exec tcp listening on {addr}");
    loop {
        let (socket, _) = listener.accept().await?;
        let store = store.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_conn(store, socket).await {
                eprintln!("git-exec tcp: {e}");
            }
        });
    }
}

async fn handle_conn(store: Arc<Store>, socket: TcpStream) -> Result<()> {
    let (read_half, mut write_half) = socket.into_split();
    let mut reader = BufReader::new(read_half);

    let mut header = String::new();
    reader.read_line(&mut header).await?;
    let header = header.trim_end();
    let mut parts = header.splitn(2, ' ');
    let (Some(verb), Some(name)) = (parts.next(), parts.next()) else {
        bail!("bad header {header:?}");
    };
    if verb != "upload-pack" && verb != "receive-pack" {
        bail!("bad verb {verb:?}");
    }
    let repo_path = store.path(name)?;
    let repo_path_str = repo_path.to_string_lossy().to_string();

    // No --stateless-rpc: this is the interactive protocol SSH clients
    // expect, same invocation shape the pre-split sshgit.rs used locally.
    let mut child = gitcmd::spawn_piped(None, &[verb, &repo_path_str], &[])?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");

    let stdin_task = tokio::spawn(async move {
        let _ = tokio::io::copy(&mut reader, &mut stdin).await;
    });
    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf).await;
        buf
    });

    let copy_result = tokio::io::copy(&mut stdout, &mut write_half).await;
    let _ = write_half.shutdown().await;
    stdin_task.abort();

    let status = child.wait().await;
    let errbuf = stderr_task.await.unwrap_or_default();
    match status {
        Ok(s) if !s.success() => {
            eprintln!("{name} {verb}: exited {s}: {}", String::from_utf8_lossy(&errbuf));
        }
        Err(e) => eprintln!("{name} {verb}: wait error: {e}"),
        _ => {}
    }

    copy_result?;
    Ok(())
}
