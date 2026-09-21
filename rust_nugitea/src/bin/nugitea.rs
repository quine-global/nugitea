//! nugitea (app tier): speaks git's smart-HTTP and SSH protocols and runs
//! the pull/push mirror scheduler. Never touches local git paths or
//! subprocesses directly — every actual git operation is proxied to a
//! `nugitea-storaged` instance over its internal HTTP API. Run alongside
//! `nugitea-storaged`, which owns the repo-root disk.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};

use rust_nugitea::auth::{AllowAll, Authorizer};
use rust_nugitea::storage_client::StorageClient;
use rust_nugitea::{graphql, httpgit, mirror, sshgit};

#[derive(Parser)]
#[command(name = "nugitea")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the HTTP + SSH git server and the mirror scheduler.
    Serve(ServeArgs),
    /// Manage bare repos on the storage tier.
    Repo {
        #[command(subcommand)]
        cmd: RepoCmd,
    },
    /// Manage pull/push mirrors.
    Mirror {
        #[command(subcommand)]
        cmd: MirrorCmd,
    },
}

#[derive(Args)]
struct ServeArgs {
    #[arg(long, default_value = ":3080")]
    http: String,
    #[arg(long, default_value = ":2222")]
    ssh: String,
    #[arg(long, default_value = "./ssh_host_ed25519")]
    ssh_host_key: PathBuf,
    #[arg(long)]
    storage: Option<String>,
    #[arg(long)]
    storage_tcp: Option<String>,
    /// Where the app tier keeps its own state (currently just
    /// mirrors.json — schedule bookkeeping, not git data).
    #[arg(long)]
    state_dir: Option<PathBuf>,
}

#[derive(Subcommand)]
enum RepoCmd {
    Create {
        name: String,
        #[arg(long)]
        storage: Option<String>,
    },
    List {
        #[arg(long)]
        storage: Option<String>,
    },
}

#[derive(Subcommand)]
enum MirrorCmd {
    AddPull {
        #[arg(long, default_value = "5m")]
        interval: String,
        #[arg(long)]
        storage: Option<String>,
        #[arg(long)]
        state_dir: Option<PathBuf>,
        repo: String,
        url: String,
    },
    AddPush {
        #[arg(long, default_value = "5m")]
        interval: String,
        #[arg(long)]
        storage: Option<String>,
        #[arg(long)]
        state_dir: Option<PathBuf>,
        repo: String,
        url: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Cmd::Serve(args) => cmd_serve(args).await,
        Cmd::Repo { cmd } => cmd_repo(cmd).await,
        Cmd::Mirror { cmd } => cmd_mirror(cmd).await,
    }
}

async fn cmd_serve(args: ServeArgs) -> Result<()> {
    let storage = Arc::new(StorageClient::new(storage_addr(args.storage), storage_tcp_addr(args.storage_tcp)));
    let auth: Arc<dyn Authorizer> = Arc::new(AllowAll);
    let mirrors = Arc::new(mirror::Store::new(&state_dir(args.state_dir))?);
    let scheduler = mirror::Scheduler::new(storage.clone(), mirrors);

    let http_addr = normalize_addr(&args.http)?;
    let ssh_addr = normalize_addr(&args.ssh)?;

    let state = httpgit::AppState {
        storage: storage.clone(),
        auth: auth.clone(),
    };
    let app = httpgit::router(state.clone()).merge(graphql::router(state));

    let sched_task = tokio::spawn(scheduler.run());

    let http_task = tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(http_addr).await?;
        println!("http: listening on {http_addr}");
        axum::serve(listener, app).await
    });

    let ssh_task = tokio::spawn(sshgit::serve(storage.clone(), auth.clone(), ssh_addr, args.ssh_host_key));

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            println!("shutting down");
        }
        res = http_task => { res??; }
        res = ssh_task => { res??; }
        res = sched_task => {
            if let Err(e) = res {
                eprintln!("mirror scheduler task ended unexpectedly: {e}");
            }
        }
    }
    Ok(())
}

async fn cmd_repo(cmd: RepoCmd) -> Result<()> {
    match cmd {
        RepoCmd::Create { name, storage } => {
            let storage = StorageClient::new(storage_addr(storage), storage_tcp_addr(None));
            storage.init_bare(&name).await?;
            println!("{name}");
        }
        RepoCmd::List { storage } => {
            let storage = StorageClient::new(storage_addr(storage), storage_tcp_addr(None));
            for name in storage.list().await? {
                println!("{name}");
            }
        }
    }
    Ok(())
}

async fn cmd_mirror(cmd: MirrorCmd) -> Result<()> {
    match cmd {
        MirrorCmd::AddPull { interval, storage, state_dir: sd, repo, url } => {
            let storage = StorageClient::new(storage_addr(storage), storage_tcp_addr(None));
            let mirrors = mirror::Store::new(&state_dir(sd))?;
            mirror::add_pull(&storage, &mirrors, &repo, &url, parse_duration(&interval)?).await
        }
        MirrorCmd::AddPush { interval, storage, state_dir: sd, repo, url } => {
            let storage = StorageClient::new(storage_addr(storage), storage_tcp_addr(None));
            let mirrors = mirror::Store::new(&state_dir(sd))?;
            mirror::add_push(&storage, &mirrors, &repo, &url, parse_duration(&interval)?).await
        }
    }
}

/// Resolves the storage tier's HTTP API base URL: an explicit `--storage`
/// flag, then `NUGITEA_STORAGE`, then a localhost default for single-node
/// dev.
fn storage_addr(flag: Option<String>) -> String {
    flag.or_else(|| std::env::var("NUGITEA_STORAGE").ok())
        .unwrap_or_else(|| "http://127.0.0.1:9080".to_string())
}

/// Resolves the storage tier's raw git-exec TCP address (used only by
/// `serve`, for the SSH transport): an explicit flag, then
/// `NUGITEA_STORAGE_TCP`, then a localhost default.
fn storage_tcp_addr(flag: Option<String>) -> String {
    flag.or_else(|| std::env::var("NUGITEA_STORAGE_TCP").ok())
        .unwrap_or_else(|| "127.0.0.1:9081".to_string())
}

/// Resolves the app tier's local state directory (currently just
/// mirrors.json): an explicit `--state-dir` flag, then `NUGITEA_STATE_DIR`,
/// then `./data` for single-node dev.
fn state_dir(flag: Option<PathBuf>) -> PathBuf {
    flag.or_else(|| std::env::var("NUGITEA_STATE_DIR").ok().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("./data"))
}

/// Accepts Go-`net.Listen`-style addresses (":3080" meaning all
/// interfaces) in addition to plain host:port.
fn normalize_addr(s: &str) -> Result<SocketAddr> {
    let s = match s.strip_prefix(':') {
        Some(port) => format!("0.0.0.0:{port}"),
        None => s.to_string(),
    };
    s.parse().with_context(|| format!("invalid address {s:?}"))
}

/// Parses a single-unit duration like "5m", "30s", "2h". A minimal
/// subset of Go's flag.Duration syntax — enough for this tool's needs.
fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim();
    if s.is_empty() {
        bail!("invalid duration: empty");
    }
    let (num, unit) = s.split_at(s.len() - 1);
    let n: u64 = num.parse().with_context(|| format!("invalid duration {s:?}"))?;
    let secs = match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        _ => bail!("invalid duration {s:?}: expected a suffix of s, m, or h"),
    };
    Ok(Duration::from_secs(secs))
}
