//! The app tier's git smart-HTTP endpoint. This is a thin reverse proxy —
//! it validates the repo name, checks `Authorizer`, and forwards the
//! request to the storage tier's internal API (`storage_server`), which is
//! where the actual git subprocess and local disk access now live.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Path, Query, Request, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};

use crate::auth::Authorizer;
use crate::service::Service;
use crate::storage_client::StorageClient;

#[derive(Clone)]
pub struct AppState {
    pub storage: Arc<StorageClient>,
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

async fn info_refs(
    State(state): State<AppState>,
    Path(repo_git): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let Some(service) = params
        .get("service")
        .and_then(|s| s.strip_prefix("git-"))
        .and_then(Service::from_http_param)
    else {
        return (
            StatusCode::BAD_REQUEST,
            "smart HTTP only: service must be git-upload-pack or git-receive-pack",
        )
            .into_response();
    };
    let Some(repo_name) = state.storage.resolve(&repo_git).await else {
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

async fn upload_pack(state: State<AppState>, path: Path<String>, headers: HeaderMap, req: Request) -> Response {
    service_handler(state, path, headers, req, Service::UploadPack).await
}

async fn receive_pack(state: State<AppState>, path: Path<String>, headers: HeaderMap, req: Request) -> Response {
    service_handler(state, path, headers, req, Service::ReceivePack).await
}

async fn service_handler(
    State(state): State<AppState>,
    Path(repo_git): Path<String>,
    headers: HeaderMap,
    req: Request,
    service: Service,
) -> Response {
    let Some(repo_name) = state.storage.resolve(&repo_git).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !service.allow(state.auth.as_ref(), &repo_name) {
        return StatusCode::FORBIDDEN.into_response();
    }

    let git_protocol = headers
        .get("Git-Protocol")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let is_gzip = headers
        .get(header::CONTENT_ENCODING)
        .map(|v| v.as_bytes() == b"gzip")
        .unwrap_or(false);
    let body = reqwest::Body::wrap_stream(req.into_body().into_data_stream());

    match state
        .storage
        .stream_git(&repo_name, service, git_protocol.as_deref(), is_gzip, body)
        .await
    {
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
