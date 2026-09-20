//! nugitea is a minimal git server: no users, no auth, no web UI — just
//! smart-HTTP and SSH clone/push against bare repos, plus pull/push
//! mirroring, implemented by shelling out to git the same way Gitea does.
//! This is the tokio/Rust rewrite; see the sibling Go implementation for
//! the original.

mod auth;
mod gitcmd;
mod httpgit;
mod mirror;
mod repo;
mod sshgit;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};

use auth::{AllowAll, Authorizer};
use repo::Store;

#[derive(Parser)]
#[command(name = "nugitea")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the HTTP + SSH git server.
    Serve(ServeArgs),
    /// Manage bare repos.
    Repo {
        #[command(subcommand)]
        cmd: RepoCmd,
    },
    /// Manage pull/push mirrors.
    Mirror {
        #[command(subcommand)]
        cmd: MirrorCmd,
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
    #[arg(long, default_value = ":3080")]
    http: String,
    #[arg(long, default_value = ":2222")]
    ssh: String,
    #[arg(long, default_value = "./data")]
    repo_root: PathBuf,
    #[arg(long, default_value = "./ssh_host_ed25519")]
    ssh_host_key: PathBuf,
}

#[derive(Subcommand)]
enum RepoCmd {
    Create { name: String },
    List,
}

#[derive(Subcommand)]
enum MirrorCmd {
    AddPull {
        #[arg(long, default_value = "5m")]
        interval: String,
        repo: String,
        url: String,
    },
    AddPush {
        #[arg(long, default_value = "5m")]
        interval: String,
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
        Cmd::Hook { name, rest } => cmd_hook(name, rest).await,
    }
}

async fn cmd_serve(args: ServeArgs) -> Result<()> {
    let repos = Arc::new(Store::new(&args.repo_root)?);
    let auth: Arc<dyn Authorizer> = Arc::new(AllowAll);
    let mirrors = Arc::new(mirror::Store::new(&repos.root));
    let scheduler = mirror::Scheduler::new(repos.clone(), mirrors);

    let http_addr = normalize_addr(&args.http)?;
    let ssh_addr = normalize_addr(&args.ssh)?;

    let state = httpgit::AppState {
        repos: repos.clone(),
        auth: auth.clone(),
    };
    let app = httpgit::router(state);

    let sched_task = tokio::spawn(scheduler.run());

    let http_task = tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(http_addr).await?;
        println!("http: listening on {http_addr}");
        axum::serve(listener, app).await
    });

    let ssh_task = tokio::spawn(sshgit::serve(
        repos.clone(),
        auth.clone(),
        ssh_addr,
        args.ssh_host_key,
    ));

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
    let repos = Store::new(repo_root_from_env())?;
    match cmd {
        RepoCmd::Create { name } => {
            let path = repos.init_bare(&name).await?;
            println!("{}", path.display());
        }
        RepoCmd::List => {
            for name in repos.list()? {
                println!("{name}");
            }
        }
    }
    Ok(())
}

async fn cmd_mirror(cmd: MirrorCmd) -> Result<()> {
    let repos = Store::new(repo_root_from_env())?;
    let mirrors = mirror::Store::new(&repos.root);
    match cmd {
        MirrorCmd::AddPull { interval, repo, url } => {
            mirror::add_pull(&repos, &mirrors, &repo, &url, parse_duration(&interval)?).await
        }
        MirrorCmd::AddPush { interval, repo, url } => {
            mirror::add_push(&repos, &mirrors, &repo, &url, parse_duration(&interval)?).await
        }
    }
}

/// Implements the `nugitea hook <name>` entry point that installed git hook
/// delegator scripts call back into. There is no policy to enforce yet (no
/// protected branches, no webhooks) — this drains stdin so it plays nicely
/// with git's hook protocol and exits 0, leaving a stub for future
/// extension.
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
    let n: u64 = num
        .parse()
        .with_context(|| format!("invalid duration {s:?}"))?;
    let secs = match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        _ => bail!("invalid duration {s:?}: expected a suffix of s, m, or h"),
    };
    Ok(Duration::from_secs(secs))
}
