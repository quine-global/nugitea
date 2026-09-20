//! nugitea-storaged (storage tier): owns the repo-root disk, runs git
//! subprocesses, and installs hooks. Exposes an internal HTTP API
//! (`storage_server`) that the app tier (`nugitea`) proxies git protocol
//! traffic through — this binary never speaks git-over-HTTP or
//! git-over-SSH to real clients itself.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Result};
use clap::{Args, Parser, Subcommand};

use rust_nugitea::storage::Store;
use rust_nugitea::{git_exec_tcp, storage_server};

#[derive(Parser)]
#[command(name = "nugitea-storaged")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the internal storage API.
    Serve(ServeArgs),
    /// Manage bare repos directly on this node (bypasses the HTTP API).
    Repo {
        #[command(subcommand)]
        cmd: RepoCmd,
    },
    /// Internal: invoked by installed git hooks.
    Hook {
        name: String,
        #[arg(trailing_var_arg = true)]
        rest: Vec<String>,
    },
}

#[derive(Args)]
struct ServeArgs {
    #[arg(long, default_value = "127.0.0.1:9080")]
    http: String,
    /// Raw TCP relay for the SSH transport's interactive git protocol
    /// (upload-pack/receive-pack without --stateless-rpc). See
    /// `git_exec_tcp` for why this can't just be another HTTP endpoint.
    #[arg(long, default_value = "127.0.0.1:9081")]
    tcp: String,
    #[arg(long, default_value = "./data")]
    repo_root: PathBuf,
}

#[derive(Subcommand)]
enum RepoCmd {
    Create { name: String },
    List,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Cmd::Serve(args) => cmd_serve(args).await,
        Cmd::Repo { cmd } => cmd_repo(cmd).await,
        Cmd::Hook { name, rest } => cmd_hook(name, rest).await,
    }
}

async fn cmd_serve(args: ServeArgs) -> Result<()> {
    let store = Arc::new(Store::new(&args.repo_root)?);
    let http_addr: std::net::SocketAddr = args
        .http
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid address {:?}: {e}", args.http))?;
    let tcp_addr: std::net::SocketAddr = args
        .tcp
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid address {:?}: {e}", args.tcp))?;

    let http_store = store.clone();
    let http_task = tokio::spawn(async move {
        let app = storage_server::router(http_store);
        let listener = tokio::net::TcpListener::bind(http_addr).await?;
        println!("storage: listening on {http_addr}");
        axum::serve(listener, app).await
    });
    let tcp_task = tokio::spawn(git_exec_tcp::serve(store, tcp_addr));

    tokio::select! {
        _ = tokio::signal::ctrl_c() => { println!("shutting down"); }
        res = http_task => { res??; }
        res = tcp_task => { res??; }
    }
    Ok(())
}

async fn cmd_repo(cmd: RepoCmd) -> Result<()> {
    let store = Store::new(repo_root_from_env())?;
    match cmd {
        RepoCmd::Create { name } => {
            let path = store.init_bare(&name).await?;
            println!("{}", path.display());
        }
        RepoCmd::List => {
            for name in store.list()? {
                println!("{name}");
            }
        }
    }
    Ok(())
}

/// Implements the `nugitea-storaged hook <name>` entry point that installed
/// git hook delegator scripts call back into. There is no policy to
/// enforce yet (no protected branches, no webhooks) — this drains stdin so
/// it plays nicely with git's hook protocol and exits 0, leaving a stub
/// for future extension.
async fn cmd_hook(name: String, _rest: Vec<String>) -> Result<()> {
    match name.as_str() {
        "pre-receive" | "post-receive" => {
            use tokio::io::AsyncReadExt;
            let mut buf = Vec::new();
            tokio::io::stdin().read_to_end(&mut buf).await?;
        }
        "update" => {
            // git invokes this as `update <refname> <oldrev> <newrev>`, no stdin
        }
        other => bail!("unknown hook {other:?}"),
    }
    Ok(())
}

fn repo_root_from_env() -> PathBuf {
    std::env::var("NUGITEA_REPO_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./data"))
}
