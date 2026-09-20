//! Implements the git smart-HTTP protocol (info/refs discovery,
//! git-upload-pack, git-receive-pack), mirroring the subprocess invocation
//! Gitea uses in routers/web/repo/githttp.go: git run with --stateless-rpc,
//! stdin/stdout wired directly to the HTTP request/response body streams.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use async_compression::tokio::bufread::GzipDecoder;
use axum::{
    body::Body,
    extract::{Path, Query, Request, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use futures::StreamExt;
use tokio_util::io::{ReaderStream, StreamReader};

use crate::auth::Authorizer;
use crate::gitcmd;
use crate::repo::Store;

#[derive(Clone)]
pub struct AppState {
    pub repos: Arc<Store>,
    pub auth: Arc<dyn Authorizer>,
}

/// Serves the smart-HTTP git protocol under paths of the form
/// /{repo}.git/info/refs, /{repo}.git/git-upload-pack,
/// /{repo}.git/git-receive-pack.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/{repo}/info/refs", get(info_refs))
        .route("/{repo}/git-upload-pack", post(upload_pack))
        .route("/{repo}/git-receive-pack", post(receive_pack))
        .with_state(state)
}

fn repo_name_from_param(raw: &str) -> Option<String> {
    let name = raw.strip_suffix(".git")?;
    Store::validate_name(name).ok()?;
    Some(name.to_string())
}

/// Matches the values git actually sends for Git-Protocol, e.g.
/// "version=2", to avoid forwarding arbitrary header content into the
/// subprocess environment.
fn is_safe_protocol_header(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '=' | ';' | ',' | '.' | '_' | '-'))
}

async fn info_refs(
    State(state): State<AppState>,
    Path(repo_git): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let Some(repo_name) = repo_name_from_param(&repo_git) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !state.repos.exists(&repo_name) {
        return StatusCode::NOT_FOUND.into_response();
    }

    let Some(service) = params
        .get("service")
        .and_then(|s| s.strip_prefix("git-"))
        .map(str::to_string)
    else {
        return (
            StatusCode::BAD_REQUEST,
            "smart HTTP only: service must be git-upload-pack or git-receive-pack",
        )
            .into_response();
    };
    let allowed = match service.as_str() {
        "upload-pack" => state.auth.allow_pull(&repo_name),
        "receive-pack" => state.auth.allow_push(&repo_name),
        _ => return (StatusCode::BAD_REQUEST, "unknown service").into_response(),
    };
    if !allowed {
        return StatusCode::FORBIDDEN.into_response();
    }

    let Ok(repo_path) = state.repos.path(&repo_name) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let output: Result<Vec<u8>> = gitcmd::run_captured(
        &repo_path,
        &[service.as_str(), "--stateless-rpc", "--advertise-refs", "."],
    )
    .await;
    let stdout = match output {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("info/refs {repo_name} {service}: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let pkt = format!("# service=git-{service}\n");
    let mut body = format!("{:04x}{pkt}0000", pkt.len() + 4).into_bytes();
    body.extend_from_slice(&stdout);

    (
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                format!("application/x-git-{service}-advertisement"),
            ),
            (header::CACHE_CONTROL, "no-cache".to_string()),
        ],
        body,
    )
        .into_response()
}

async fn upload_pack(
    state: State<AppState>,
    path: Path<String>,
    headers: HeaderMap,
    req: Request,
) -> Response {
    service_handler(state, path, headers, req, "upload-pack", |a, r| {
        a.allow_pull(r)
    })
    .await
}

async fn receive_pack(
    state: State<AppState>,
    path: Path<String>,
    headers: HeaderMap,
    req: Request,
) -> Response {
    service_handler(state, path, headers, req, "receive-pack", |a, r| {
        a.allow_push(r)
    })
    .await
}

async fn service_handler(
    State(state): State<AppState>,
    Path(repo_git): Path<String>,
    headers: HeaderMap,
    req: Request,
    service: &'static str,
    allow: fn(&dyn Authorizer, &str) -> bool,
) -> Response {
    let Some(repo_name) = repo_name_from_param(&repo_git) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !state.repos.exists(&repo_name) {
        return StatusCode::NOT_FOUND.into_response();
    }
    if !allow(state.auth.as_ref(), &repo_name) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(repo_path) = state.repos.path(&repo_name) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let mut extra_env = Vec::new();
    if let Some(proto) = headers.get("Git-Protocol").and_then(|v| v.to_str().ok()) {
        if is_safe_protocol_header(proto) {
            extra_env.push(("GIT_PROTOCOL".to_string(), proto.to_string()));
        }
    }

    let mut child = match gitcmd::spawn_piped(Some(&repo_path), &[service, "--stateless-rpc", "."], &extra_env) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("spawn {service}: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");

    let is_gzip = headers
        .get(header::CONTENT_ENCODING)
        .map(|v| v.as_bytes() == b"gzip")
        .unwrap_or(false);
    let body_stream = req.into_body().into_data_stream();

    tokio::spawn(async move {
        let mapped = body_stream.map(|r| r.map_err(std::io::Error::other));
        let reader = StreamReader::new(mapped);
        if is_gzip {
            let mut decoder = GzipDecoder::new(tokio::io::BufReader::new(reader));
            let _ = tokio::io::copy(&mut decoder, &mut stdin).await;
        } else {
            let mut reader = reader;
            let _ = tokio::io::copy(&mut reader, &mut stdin).await;
        }
        drop(stdin);
    });

    let repo_for_log = repo_name.clone();
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut errbuf = Vec::new();
        let _ = stderr.read_to_end(&mut errbuf).await;
        match child.wait().await {
            Ok(status) if !status.success() => {
                eprintln!(
                    "{repo_for_log} {service}: exited {status}: {}",
                    String::from_utf8_lossy(&errbuf)
                );
            }
            Err(e) => eprintln!("{repo_for_log} {service}: wait error: {e}"),
            _ => {}
        }
    });

    Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            format!("application/x-git-{service}-result"),
        )
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(ReaderStream::new(stdout)))
        .unwrap()
}
