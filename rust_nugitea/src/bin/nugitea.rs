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

use rust_nugitea::accounts::{self, Visibility};
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
    /// Manage user accounts.
    User {
        #[command(subcommand)]
        cmd: UserCmd,
    },
    /// Manage orgs, including nested ones (`acme/platform`).
    Org {
        #[command(subcommand)]
        cmd: OrgCmd,
    },
    /// Manage repos: an entry in accounts.json plus a bare repo on the
    /// storage tier.
    Repo {
        #[command(subcommand)]
        cmd: RepoCmd,
    },
    /// Manage pull/push mirrors.
    Mirror {
        #[command(subcommand)]
        cmd: MirrorCmd,
    },
    /// Print the GraphQL schema as SDL (see web/schema.graphql).
    GraphqlSchema,
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
    /// Where the app tier keeps its own state: accounts.json (users,
    /// orgs, repo ownership) and mirrors.json (schedule bookkeeping) —
    /// everything but git data.
    #[arg(long)]
    state_dir: Option<PathBuf>,
}

#[derive(Subcommand)]
enum UserCmd {
    Create {
        login: String,
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },
    List {
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum OrgCmd {
    /// Create an org; `parent/child` creates a sub-org under an existing
    /// org.
    Create {
        path: String,
        #[arg(long, default_value = "public", value_parser = parse_visibility)]
        visibility: Visibility,
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },
    List {
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum RepoCmd {
    /// Create `owner/name` (or `org/sub-org/.../name`) under an existing
    /// user or org.
    Create {
        path: String,
        #[arg(long, default_value = "public", value_parser = parse_visibility)]
        visibility: Visibility,
        #[arg(long)]
        storage: Option<String>,
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },
    List {
        #[arg(long)]
        state_dir: Option<PathBuf>,
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
        Cmd::User { cmd } => cmd_user(cmd).await,
        Cmd::Org { cmd } => cmd_org(cmd).await,
        Cmd::Repo { cmd } => cmd_repo(cmd).await,
        Cmd::Mirror { cmd } => cmd_mirror(cmd).await,
        Cmd::GraphqlSchema => {
            print!("{}", graphql::sdl());
            Ok(())
        }
    }
}

async fn cmd_serve(args: ServeArgs) -> Result<()> {
    let storage = Arc::new(StorageClient::new(storage_addr(args.storage), storage_tcp_addr(args.storage_tcp)));
    let auth: Arc<dyn Authorizer> = Arc::new(AllowAll);
    let state_dir = state_dir(args.state_dir);
    let accounts = Arc::new(accounts::Store::new(&state_dir)?);
    let mirrors = Arc::new(mirror::Store::new(&state_dir)?);
    let scheduler = mirror::Scheduler::new(storage.clone(), mirrors);

    let http_addr = normalize_addr(&args.http)?;
    let ssh_addr = normalize_addr(&args.ssh)?;

    let state = httpgit::AppState { storage, auth, accounts };
    let app = httpgit::router(state.clone()).merge(graphql::router(state.clone()));

    let sched_task = tokio::spawn(scheduler.run());

    let http_task = tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(http_addr).await?;
        println!("http: listening on {http_addr}");
        axum::serve(listener, app).await
    });

    let ssh_task = tokio::spawn(sshgit::serve(state, ssh_addr, args.ssh_host_key));

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

async fn cmd_user(cmd: UserCmd) -> Result<()> {
    match cmd {
        UserCmd::Create { login, state_dir: sd } => {
            let accounts = accounts::Store::new(&state_dir(sd))?;
            accounts.update(|d| d.add_user(&login)).await?;
            println!("{login}");
        }
        UserCmd::List { state_dir: sd } => print_accounts(sd, false).await?,
    }
    Ok(())
}

async fn cmd_org(cmd: OrgCmd) -> Result<()> {
    match cmd {
        OrgCmd::Create { path, visibility, state_dir: sd } => {
            let accounts = accounts::Store::new(&state_dir(sd))?;
            let id = accounts.update(|d| d.add_org_at(&path, visibility)).await?;
            println!("{}", accounts.load().await?.path(id).unwrap_or(path));
        }
        OrgCmd::List { state_dir: sd } => print_accounts(sd, true).await?,
    }
    Ok(())
}

async fn print_accounts(sd: Option<PathBuf>, orgs: bool) -> Result<()> {
    let dir = accounts::Store::new(&state_dir(sd))?.load().await?;
    let mut paths: Vec<String> = dir.accounts().filter(|a| a.is_org() == orgs).filter_map(|a| dir.path(a.id)).collect();
    paths.sort();
    for p in paths {
        println!("{p}");
    }
    Ok(())
}

async fn cmd_repo(cmd: RepoCmd) -> Result<()> {
    match cmd {
        RepoCmd::Create { path, visibility, storage, state_dir: sd } => {
            let storage = StorageClient::new(storage_addr(storage), storage_tcp_addr(None));
            let accounts = accounts::Store::new(&state_dir(sd))?;
            // Validate against the directory (owner exists, name free,
            // visibility allowed) before touching the storage tier, then
            // record it only once the bare repo actually exists there.
            let mut dry_run = accounts.load().await?;
            let id = dry_run.add_repo_at(&path, visibility)?;
            let canonical = dry_run.repo_path(id).expect("just added");
            storage.init_bare(&canonical).await?;
            accounts.update(|d| d.add_repo_at(&canonical, visibility)).await?;
            println!("{canonical}");
        }
        RepoCmd::List { state_dir: sd } => {
            let dir = accounts::Store::new(&state_dir(sd))?.load().await?;
            let mut paths: Vec<String> = dir.repos().filter_map(|r| dir.repo_path(r.id)).collect();
            paths.sort();
            for p in paths {
                println!("{p}");
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
    flag.or_else(|| std::env::var("NUGITEA_STORAGE").ok()).unwrap_or_else(|| "http://127.0.0.1:9080".to_string())
}

/// Resolves the storage tier's raw git-exec TCP address (used only by
/// `serve`, for the SSH transport): an explicit flag, then
/// `NUGITEA_STORAGE_TCP`, then a localhost default.
fn storage_tcp_addr(flag: Option<String>) -> String {
    flag.or_else(|| std::env::var("NUGITEA_STORAGE_TCP").ok()).unwrap_or_else(|| "127.0.0.1:9081".to_string())
}

/// Resolves the app tier's local state directory (accounts.json,
/// mirrors.json): an explicit `--state-dir` flag, then `NUGITEA_STATE_DIR`,
/// then `./data` for single-node dev.
fn state_dir(flag: Option<PathBuf>) -> PathBuf {
    flag.or_else(|| std::env::var("NUGITEA_STATE_DIR").ok().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("./data"))
}

fn parse_visibility(s: &str) -> Result<Visibility> {
    serde_json::from_value(serde_json::Value::String(s.to_string()))
        .with_context(|| format!("invalid visibility {s:?}: expected public, internal, or private"))
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
