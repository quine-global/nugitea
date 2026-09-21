//! A GitHub-schema-flavored GraphQL API for browsing repo contents — the
//! app tier's replacement for the old Leptos SSR pages, now that browsing
//! UI lives in a separate Nuxt.js app (`web/`) that queries this.
//!
//! GitHub's real schema addresses tree/blob content via
//! `repository.object(expression: "REF:PATH")`, returning a `GitObject`
//! interface resolved with inline fragments (`... on Blob`, `... on
//! Tree`). That maps almost exactly onto the `git_ref:path` addressing
//! `tree::list_tree`/`read_blob` already use on the storage tier, so this
//! reuses that shape rather than inventing a new one — same idea, but the
//! `files(ref:)` field (a full recursive file list) isn't literal GitHub
//! API shape; it's a pragmatic reuse of `tree::list_all_files`, kept
//! because that's what the search feature needs.
//!
//! No auth beyond the usual: `Query.repository` checks
//! `Authorizer::allow_pull` once and returns `null` for anything not
//! allowed or not found, matching how GitHub itself returns `null` for a
//! repo you can't see rather than a different error shape — resolvers
//! below it don't need to re-check.

use async_graphql::{http::GraphiQLSource, Context, EmptyMutation, EmptySubscription, Error, Interface, Object, Result, Schema, SimpleObject};
use async_graphql_axum::GraphQL;
use axum::{
    response::{Html, IntoResponse},
    routing::post_service,
    Router,
};
use tower_http::cors::CorsLayer;

use crate::httpgit::AppState;
use crate::tree::EntryKind;

type NugiteaSchema = Schema<Query, EmptyMutation, EmptySubscription>;

pub fn router(state: AppState) -> Router {
    let schema: NugiteaSchema = Schema::build(Query, EmptyMutation, EmptySubscription)
        .data(state)
        .finish();
    Router::new()
        .route("/graphql", post_service(GraphQL::new(schema)).get(graphiql))
        .layer(CorsLayer::permissive())
}

async fn graphiql() -> impl IntoResponse {
    Html(GraphiQLSource::build().endpoint("/graphql").finish())
}

fn gql_err(e: impl std::fmt::Display) -> Error {
    Error::new(e.to_string())
}

/// Splits a GitHub-style `"ref"` or `"ref:path"` expression.
fn split_expression(expression: &str) -> (&str, &str) {
    match expression.split_once(':') {
        Some((r, p)) => (r, p),
        None => (expression, ""),
    }
}

pub struct Query;

#[Object]
impl Query {
    async fn repository(&self, ctx: &Context<'_>, name: String) -> Result<Option<Repository>> {
        let state = ctx.data::<AppState>()?;
        if !state.auth.allow_pull(&name) {
            return Ok(None);
        }
        if !state.storage.exists(&name).await {
            return Ok(None);
        }
        Ok(Some(Repository { name }))
    }
}

pub struct Repository {
    name: String,
}

#[Object]
impl Repository {
    async fn name(&self) -> &str {
        &self.name
    }

    /// `main` if it exists, else `master`, else whichever branch sorts
    /// first, else `null` — the same heuristic the old Leptos UI used,
    /// since HEAD in a freshly created bare repo doesn't reliably reflect
    /// what was actually pushed.
    async fn default_branch_ref(&self, ctx: &Context<'_>) -> Result<Option<Ref>> {
        let state = ctx.data::<AppState>()?;
        let branches = state.storage.branches(&self.name).await.map_err(gql_err)?;
        let default = branches
            .iter()
            .find(|b| b.as_str() == "main")
            .or_else(|| branches.iter().find(|b| b.as_str() == "master"))
            .or_else(|| branches.iter().min())
            .cloned();
        Ok(default.map(|name| Ref { name }))
    }

    async fn refs(&self, ctx: &Context<'_>) -> Result<Vec<Ref>> {
        let state = ctx.data::<AppState>()?;
        Ok(state.storage.branches(&self.name).await.map_err(gql_err)?.into_iter().map(|name| Ref { name }).collect())
    }

    /// Full recursive file list for the search feature — see the module
    /// doc comment for why this isn't literal GitHub API shape.
    async fn files(&self, ctx: &Context<'_>, r#ref: String) -> Result<Vec<String>> {
        let state = ctx.data::<AppState>()?;
        state.storage.all_files(&self.name, &r#ref).await.map_err(gql_err)
    }

    /// Resolves a `"ref"` or `"ref:path"` expression to a `Tree` or
    /// `Blob`. Tried as a tree first (a no-op-cheap `git ls-tree`); if
    /// that fails on a non-empty path, it's tried as a blob instead — a
    /// path can't be both, so git itself is the source of truth on which
    /// one an expression resolves to, rather than nugitea guessing from
    /// the shape of the path string.
    async fn object(&self, ctx: &Context<'_>, expression: String) -> Result<Option<GitObject>> {
        let state = ctx.data::<AppState>()?;
        let (git_ref, path) = split_expression(&expression);

        if let Ok(entries) = state.storage.tree(&self.name, git_ref, path).await {
            let entries = entries
                .into_iter()
                .map(|e| TreeEntry {
                    name: e.name,
                    r#type: match e.kind {
                        EntryKind::Tree => "tree".to_string(),
                        EntryKind::Blob => "blob".to_string(),
                    },
                    oid: e.sha,
                })
                .collect();
            // Not wired to a real `git rev-parse` — nothing here consumes
            // a tree's own oid (only its entries' oids, which are real),
            // so it's left blank rather than adding a subprocess call and
            // a new storage-tier endpoint for a value nothing reads yet.
            return Ok(Some(GitObject::Tree(Tree { oid: String::new(), entries })));
        }
        if path.is_empty() {
            return Ok(None); // root of a nonexistent ref is neither a tree nor a blob
        }
        match state.storage.blob(&self.name, git_ref, path).await {
            Ok(blob) => Ok(Some(GitObject::Blob(Blob {
                oid: blob.sha,
                is_binary: blob.binary,
                byte_size: blob.content.len() as i32,
                text: if blob.binary { None } else { Some(blob.content) },
            }))),
            Err(_) => Ok(None),
        }
    }
}

#[derive(SimpleObject)]
pub struct Ref {
    name: String,
}

#[derive(Interface)]
#[graphql(field(name = "oid", ty = "String"))]
pub enum GitObject {
    Tree(Tree),
    Blob(Blob),
}

#[derive(SimpleObject)]
pub struct Tree {
    oid: String,
    entries: Vec<TreeEntry>,
}

#[derive(SimpleObject)]
pub struct TreeEntry {
    name: String,
    #[graphql(name = "type")]
    r#type: String,
    oid: String,
}

#[derive(SimpleObject)]
pub struct Blob {
    oid: String,
    is_binary: bool,
    text: Option<String>,
    byte_size: i32,
}
