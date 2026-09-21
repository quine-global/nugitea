//! The storage tier's internal HTTP API: owns the repo-root disk, runs git
//! subprocesses, and streams git protocol bytes to/from the app tier. This
//! is the network boundary that makes nugitea scale like GitLab+Gitaly
//! instead of like Gitea — the app tier never touches a local git path or
//! subprocess, only this API.
//!
//! No auth here: this service trusts its caller (the app tier), the same
//! way Gitaly trusts the Rails app tier. `Authorizer` checks already
//! happened once, at the app tier, before a request is ever proxied here.

use std::collections::HashMap;
use std::sync::Arc;

use async_compression::tokio::bufread::GzipDecoder;
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, head, post},
    Json, Router,
};
use futures::StreamExt;
use serde::Deserialize;
use tokio_util::io::{ReaderStream, StreamReader};

use crate::gitcmd;
use crate::service::Service;
use crate::storage::Store;
use crate::tree;

pub fn router(store: Arc<Store>) -> Router {
    Router::new()
        .route("/repos", get(list_repos))
        .route("/repos/{name}", head(exists_repo).post(create_repo))
        .route("/repos/{name}/info-refs", get(info_refs))
        .route("/repos/{name}/upload-pack", post(upload_pack))
        .route("/repos/{name}/receive-pack", post(receive_pack))
        .route("/repos/{name}/git", post(run_git))
        .route("/repos/{name}/branches", get(branches))
        .route("/repos/{name}/files/{ref}", get(files))
        .route("/repos/{name}/tree/{ref}", get(tree_root))
        .route("/repos/{name}/tree/{ref}/{*path}", get(tree_at_path))
        .route("/repos/{name}/blob/{ref}/{*path}", get(blob))
        .with_state(store)
}

/// Matches the values git actually sends for Git-Protocol, e.g.
/// "version=2", to avoid forwarding arbitrary header content into the
/// subprocess environment. Re-checked here even though the app tier
/// already validates it — don't blindly trust a "trusted" caller either.
fn is_safe_protocol_header(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '=' | ';' | ',' | '.' | '_' | '-'))
}

async fn list_repos(State(store): State<Arc<Store>>) -> Response {
    match store.list() {
        Ok(names) => Json(names).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn exists_repo(State(store): State<Arc<Store>>, Path(name): Path<String>) -> StatusCode {
    if store.exists(&name).await {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

async fn create_repo(State(store): State<Arc<Store>>, Path(name): Path<String>) -> Response {
    match store.init_bare(&name).await {
        Ok(_) => StatusCode::CREATED.into_response(),
        Err(e) => {
            let msg = e.to_string();
            let status = if msg.contains("already exists") {
                StatusCode::CONFLICT
            } else if msg.contains("invalid repo name") {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (status, msg).into_response()
        }
    }
}

async fn info_refs(
    State(store): State<Arc<Store>>,
    Path(name): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let Some(service) = params
        .get("service")
        .and_then(|s| s.strip_prefix("git-"))
        .and_then(Service::from_http_param)
    else {
        return (StatusCode::BAD_REQUEST, "unknown service").into_response();
    };
    let Ok(repo_path) = store.path(&name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !store.exists(&name).await {
        return StatusCode::NOT_FOUND.into_response();
    }

    let arg = service.git_arg();
    let stdout = match gitcmd::run_captured(&repo_path, &[arg, "--stateless-rpc", "--advertise-refs", "."]).await {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("info-refs {name} {arg}: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let pkt = format!("# service=git-{arg}\n");
    let mut body = format!("{:04x}{pkt}0000", pkt.len() + 4).into_bytes();
    body.extend_from_slice(&stdout);
    body.into_response()
}

async fn upload_pack(state: State<Arc<Store>>, path: Path<String>, headers: HeaderMap, req: axum::extract::Request) -> Response {
    stream_service(state, path, headers, req, Service::UploadPack).await
}

async fn receive_pack(state: State<Arc<Store>>, path: Path<String>, headers: HeaderMap, req: axum::extract::Request) -> Response {
    stream_service(state, path, headers, req, Service::ReceivePack).await
}

async fn stream_service(
    State(store): State<Arc<Store>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    req: axum::extract::Request,
    service: Service,
) -> Response {
    let Ok(repo_path) = store.path(&name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !store.exists(&name).await {
        return StatusCode::NOT_FOUND.into_response();
    }
    let arg = service.git_arg();

    let mut extra_env = Vec::new();
    if let Some(proto) = headers.get("Git-Protocol").and_then(|v| v.to_str().ok()) {
        if is_safe_protocol_header(proto) {
            extra_env.push(("GIT_PROTOCOL".to_string(), proto.to_string()));
        }
    }

    let mut child = match gitcmd::spawn_piped(Some(&repo_path), &[arg, "--stateless-rpc", "."], &extra_env) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("spawn {arg}: {e}");
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

    tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut errbuf = Vec::new();
        let _ = stderr.read_to_end(&mut errbuf).await;
        match child.wait().await {
            Ok(status) if !status.success() => {
                eprintln!("{name} {arg}: exited {status}: {}", String::from_utf8_lossy(&errbuf));
            }
            Err(e) => eprintln!("{name} {arg}: wait error: {e}"),
            _ => {}
        }
    });

    Response::builder()
        .status(StatusCode::OK)
        .body(Body::from_stream(ReaderStream::new(stdout)))
        .unwrap()
}

#[derive(Deserialize)]
struct RunGitRequest {
    args: Vec<String>,
}

/// Generic unary git command for mirror operations (remote add/remove,
/// fetch, push --mirror) — the only git invocations that don't need
/// request/response body streaming.
async fn run_git(State(store): State<Arc<Store>>, Path(name): Path<String>, Json(req): Json<RunGitRequest>) -> Response {
    let Ok(repo_path) = store.path(&name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let args: Vec<&str> = req.args.iter().map(String::as_str).collect();
    match gitcmd::run(&repo_path, &args).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn branches(State(store): State<Arc<Store>>, Path(name): Path<String>) -> Response {
    let Ok(repo_path) = store.path(&name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match tree::list_branches(&repo_path).await {
        Ok(names) => Json(names).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn files(State(store): State<Arc<Store>>, Path((name, git_ref)): Path<(String, String)>) -> Response {
    let Ok(repo_path) = store.path(&name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match tree::list_all_files(&repo_path, &git_ref).await {
        Ok(paths) => Json(paths).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    }
}

async fn tree_root(state: State<Arc<Store>>, Path((name, git_ref)): Path<(String, String)>) -> Response {
    tree_listing(state, name, git_ref, String::new()).await
}

async fn tree_at_path(
    state: State<Arc<Store>>,
    Path((name, git_ref, path)): Path<(String, String, String)>,
) -> Response {
    tree_listing(state, name, git_ref, path).await
}

async fn tree_listing(State(store): State<Arc<Store>>, name: String, git_ref: String, path: String) -> Response {
    let Ok(repo_path) = store.path(&name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match tree::list_tree(&repo_path, &git_ref, &path).await {
        Ok(entries) => Json(entries).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    }
}

async fn blob(
    State(store): State<Arc<Store>>,
    Path((name, git_ref, path)): Path<(String, String, String)>,
) -> Response {
    let Ok(repo_path) = store.path(&name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match tree::read_blob(&repo_path, &git_ref, &path).await {
        Ok(blob) => match String::from_utf8(blob.content) {
            Ok(content) if !content.contains('\0') => {
                Json(tree::BlobContent { content, binary: false, sha: blob.sha }).into_response()
            }
            _ => Json(tree::BlobContent { content: String::new(), binary: true, sha: blob.sha }).into_response(),
        },
        Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    }
}
