//! A minimal read-only file-browser UI, served by the app tier alongside
//! the git protocol endpoints. Every click is a normal link (full page
//! load) — there's no client-side state, so this renders with Leptos in
//! SSR-only mode: components compose into a view, `.to_html()` turns that
//! into a plain `String`, no WASM/hydration/cargo-leptos involved.
//!
//! Reuses `httpgit::AppState` — browsing is a read, gated by
//! `Authorizer::allow_pull` the same as clone.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
use leptos::prelude::*;

use crate::httpgit::AppState;
use crate::tree::{EntryKind, TreeEntry};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/{repo}", get(repo_root))
        .route("/{repo}/tree/{ref}", get(tree_root))
        .route("/{repo}/tree/{ref}/{*path}", get(tree_at_path))
        .route("/{repo}/blob/{ref}/{*path}", get(blob))
        .with_state(state)
}

const PAGE_CSS: &str = "
body { font-family: -apple-system, sans-serif; margin: 2rem; color: #222; }
.container { max-width: 60rem; margin: 0 auto; }
nav.breadcrumbs { margin-bottom: 1rem; font-size: 1.1rem; }
ul.entries { list-style: none; padding: 0; }
ul.entries li { padding: 0.25rem 0; border-bottom: 1px solid #eee; }
pre { background: #f6f8fa; padding: 1rem; overflow-x: auto; border-radius: 4px; }
a { color: #0969da; text-decoration: none; }
a:hover { text-decoration: underline; }
";

fn render_page(title: &str, body: AnyView) -> Html<String> {
    let title = title.to_string();
    let html = view! {
        <html>
            <head>
                <title>{title.clone()}</title>
                <style>{PAGE_CSS}</style>
            </head>
            <body>
                <div class="container">
                    <h2>{title}</h2>
                    {body}
                </div>
            </body>
        </html>
    }
    .to_html();
    Html(html)
}

/// Builds `(label, href)` pairs for the repo name plus each path segment.
fn breadcrumb_links(repo: &str, git_ref: &str, path: &str) -> Vec<(String, String)> {
    let mut links = vec![(repo.to_string(), format!("/{repo}/tree/{git_ref}"))];
    let mut acc = String::new();
    if !path.is_empty() {
        for seg in path.split('/') {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(seg);
            links.push((seg.to_string(), format!("/{repo}/tree/{git_ref}/{acc}")));
        }
    }
    links
}

#[component]
fn Breadcrumbs(links: Vec<(String, String)>) -> impl IntoView {
    let last = links.len().saturating_sub(1);
    view! {
        <nav class="breadcrumbs">
            {links
                .into_iter()
                .enumerate()
                .map(|(i, (label, href))| {
                    if i == last {
                        view! { <span><strong>{label}</strong></span> }.into_any()
                    } else {
                        view! { <a href=href>{label}</a>" / " }.into_any()
                    }
                })
                .collect::<Vec<_>>()}
        </nav>
    }
}

#[component]
fn DirListing(repo: String, git_ref: String, path: String, entries: Vec<TreeEntry>) -> impl IntoView {
    let mut entries = entries;
    entries.sort_by(|a, b| {
        let kind_order = |k: EntryKind| matches!(k, EntryKind::Blob) as u8;
        kind_order(a.kind).cmp(&kind_order(b.kind)).then_with(|| a.name.cmp(&b.name))
    });

    if entries.is_empty() {
        return view! { <p><em>"(empty directory)"</em></p> }.into_any();
    }

    view! {
        <ul class="entries">
            {entries
                .into_iter()
                .map(|e| {
                    let child_path = if path.is_empty() { e.name.clone() } else { format!("{path}/{}", e.name) };
                    let (href, icon) = match e.kind {
                        EntryKind::Tree => (format!("/{repo}/tree/{git_ref}/{child_path}"), "\u{1F4C1}"),
                        EntryKind::Blob => (format!("/{repo}/blob/{git_ref}/{child_path}"), "\u{1F4C4}"),
                    };
                    view! { <li>{icon}" "<a href=href>{e.name}</a></li> }
                })
                .collect::<Vec<_>>()}
        </ul>
    }
    .into_any()
}

async fn repo_root(State(state): State<AppState>, Path(repo): Path<String>) -> Response {
    if !state.auth.allow_pull(&repo) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(branches) = state.storage.branches(&repo).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let default = branches
        .iter()
        .find(|b| b.as_str() == "main")
        .or_else(|| branches.iter().find(|b| b.as_str() == "master"))
        .or_else(|| branches.iter().min())
        .cloned();

    match default {
        Some(b) => Redirect::to(&format!("/{repo}/tree/{b}")).into_response(),
        None => render_page(
            &repo,
            view! { <p>"This repository has no branches yet."</p> }.into_any(),
        )
        .into_response(),
    }
}

async fn tree_root(state: State<AppState>, Path((repo, git_ref)): Path<(String, String)>) -> Response {
    render_tree(state, repo, git_ref, String::new()).await
}

async fn tree_at_path(state: State<AppState>, Path((repo, git_ref, path)): Path<(String, String, String)>) -> Response {
    render_tree(state, repo, git_ref, path).await
}

async fn render_tree(State(state): State<AppState>, repo: String, git_ref: String, path: String) -> Response {
    if !state.auth.allow_pull(&repo) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let entries = match state.storage.tree(&repo, &git_ref, &path).await {
        Ok(e) => e,
        Err(e) => return (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    };

    let body = view! {
        <Breadcrumbs links=breadcrumb_links(&repo, &git_ref, &path) />
        <DirListing repo=repo.clone() git_ref=git_ref.clone() path=path entries=entries />
    }
    .into_any();
    render_page(&repo, body).into_response()
}

async fn blob(State(state): State<AppState>, Path((repo, git_ref, path)): Path<(String, String, String)>) -> Response {
    if !state.auth.allow_pull(&repo) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let blob = match state.storage.blob(&repo, &git_ref, &path).await {
        Ok(b) => b,
        Err(e) => return (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    };

    let content_view = if blob.binary {
        view! { <p><em>"binary file not shown"</em></p> }.into_any()
    } else {
        view! { <pre>{blob.content}</pre> }.into_any()
    };
    let body = view! {
        <Breadcrumbs links=breadcrumb_links(&repo, &git_ref, &path) />
        {content_view}
    }
    .into_any();
    render_page(&repo, body).into_response()
}
