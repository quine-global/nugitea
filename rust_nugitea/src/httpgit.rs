//! The app tier's git smart-HTTP endpoint. This is a thin reverse proxy —
//! it resolves the repo path against the accounts directory, checks
//! `Authorizer`, and forwards the
//! request to the storage tier's internal API (`storage_server`), which is
//! where the actual git subprocess and local disk access now live.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Path, Query, Request, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};

use crate::accounts;
use crate::auth::Authorizer;
use crate::service::Service;
use crate::storage_client::StorageClient;

#[derive(Clone)]
pub struct AppState {
    pub storage: Arc<StorageClient>,
    pub auth: Arc<dyn Authorizer>,
    pub accounts: Arc<accounts::Store>,
}

impl AppState {
    /// Resolves a wire-form repo path (`acme/platform/api.git`) to the
    /// canonical path of a repo that exists, or None.
    pub async fn resolve(&self, raw: &str) -> Option<String> {
        let dir = match self.accounts.load().await {
            Ok(dir) => dir,
            Err(e) => {
                eprintln!("load accounts: {e}");
                return None;
            }
        };
        self.storage.resolve(&dir, raw).await
    }
}

/// Serves the smart-HTTP git protocol under paths of the form
/// /{owner path}/{repo}.git/info/refs, .../git-upload-pack, and
/// .../git-receive-pack. The owner path can be any depth (nested orgs),
/// and axum only allows a wildcard as the last segment, so these are one
/// catch-all per method that splits off the fixed suffix itself. More
/// specific routes merged alongside (e.g. `/graphql`) still win.
pub fn router(state: AppState) -> Router {
    Router::new().route("/{*path}", get(get_git).post(post_git)).with_state(state)
}

async fn get_git(state: State<AppState>, Path(path): Path<String>, query: Query<HashMap<String, String>>) -> Response {
    match path.strip_suffix("/info/refs") {
        Some(repo_git) => info_refs(state, repo_git, query).await,
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn post_git(state: State<AppState>, Path(path): Path<String>, headers: HeaderMap, req: Request) -> Response {
    if let Some(repo_git) = path.strip_suffix("/git-upload-pack") {
        service_handler(state, repo_git, headers, req, Service::UploadPack).await
    } else if let Some(repo_git) = path.strip_suffix("/git-receive-pack") {
        service_handler(state, repo_git, headers, req, Service::ReceivePack).await
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

async fn info_refs(
    State(state): State<AppState>,
    repo_git: &str,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let Some(service) = params.get("service").and_then(|s| s.strip_prefix("git-")).and_then(Service::from_http_param)
    else {
        return (StatusCode::BAD_REQUEST, "smart HTTP only: service must be git-upload-pack or git-receive-pack")
            .into_response();
    };
    let Some(repo_name) = state.resolve(repo_git).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !service.allow(state.auth.as_ref(), &repo_name) {
        return StatusCode::FORBIDDEN.into_response();
    }

    let arg = service.git_arg();
    match state.storage.info_refs(&repo_name, service).await {
        Ok(body) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, format!("application/x-git-{arg}-advertisement")),
                (header::CACHE_CONTROL, "no-cache".to_string()),
            ],
            body,
        )
            .into_response(),
        Err(e) => {
            eprintln!("info/refs {repo_name} {arg}: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn service_handler(
    State(state): State<AppState>,
    repo_git: &str,
    headers: HeaderMap,
    req: Request,
    service: Service,
) -> Response {
    let Some(repo_name) = state.resolve(repo_git).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !service.allow(state.auth.as_ref(), &repo_name) {
        return StatusCode::FORBIDDEN.into_response();
    }

    let git_protocol = headers.get("Git-Protocol").and_then(|v| v.to_str().ok()).map(str::to_string);
    let is_gzip = headers.get(header::CONTENT_ENCODING).map(|v| v.as_bytes() == b"gzip").unwrap_or(false);
    let body = reqwest::Body::wrap_stream(req.into_body().into_data_stream());

    match state.storage.stream_git(&repo_name, service, git_protocol.as_deref(), is_gzip, body).await {
        Ok(stream) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, format!("application/x-git-{}-result", service.git_arg()))
            .header(header::CACHE_CONTROL, "no-cache")
            .body(Body::from_stream(stream))
            .unwrap(),
        Err(e) => {
            eprintln!("proxy {repo_name} {}: {e}", service.git_arg());
            (StatusCode::BAD_GATEWAY, "storage tier error").into_response()
        }
    }
}
