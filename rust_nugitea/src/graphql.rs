//! A GitHub-schema-compatible GraphQL API for browsing owners and repo
//! contents, queried by the separate Nuxt.js app (`web/`).
//!
//! The owner side follows GitHub's schema: `repository(owner:, name:)`,
//! `repositoryOwner(login:)` / `user(login:)` / `organization(login:)`
//! returning the `RepositoryOwner` interface, `resource(url:)` returning
//! the `UniformResourceLocatable` interface, and owners' `repositories`
//! as a connection (`nodes`, `totalCount`; `first` is accepted but there's
//! no cursor paging). A GitHub client works unchanged against single-level
//! owners.
//!
//! Nested orgs are the one extension. A sub-org's `login` is its full
//! path (`acme/platform`), so `repository(owner: "acme/platform", name:
//! "api")` and `resource(url: "/acme/platform/api")` just work, and
//! `Organization` gains two fields GitHub doesn't have: `parentOrganization`
//! and `subOrganizations`. The top-level `repositories` list (the `/repos`
//! page's source) is also nugitea-only.
//!
//! Contents follow GitHub's `repository.object(expression: "REF:PATH")`,
//! returning a `GitObject` interface resolved with inline fragments (`...
//! on Blob`, `... on Tree`) — that maps almost exactly onto the
//! `git_ref:path` addressing `tree::list_tree`/`read_blob` already use on
//! the storage tier. `files(ref:)` (a full recursive file list) isn't
//! literal GitHub API shape; it's kept because that's what the search
//! feature needs.
//!
//! Each query loads one snapshot of the accounts directory up front, so
//! everything in a response is consistent with itself. Anything
//! `Authorizer::allow_pull` rejects resolves to `null` or is left out of
//! lists, matching how GitHub returns `null` for a repo you can't see
//! rather than a different error shape.

use std::sync::Arc;

use async_graphql::{
    http::GraphiQLSource, Context, EmptyMutation, EmptySubscription, Enum, Error, Interface, Object, Result, Schema,
    SimpleObject, ID,
};
use async_graphql_axum::GraphQL;
use axum::{
    response::{Html, IntoResponse},
    routing::post_service,
    Router,
};
use tower_http::cors::CorsLayer;

use crate::accounts::{Directory, Id, Visibility};
use crate::auth::Authorizer;
use crate::httpgit::AppState;
use crate::tree::EntryKind;

type NugiteaSchema = Schema<Query, EmptyMutation, EmptySubscription>;

/// The schema as SDL, for `nugitea graphql-schema` — the source of the
/// frontend's checked-in `web/schema.graphql`, which editor GraphQL
/// tooling reads to autocomplete and validate queries.
pub fn sdl() -> String {
    Schema::build(Query, EmptyMutation, EmptySubscription).finish().sdl()
}

pub fn schema(state: AppState) -> NugiteaSchema {
    Schema::build(Query, EmptyMutation, EmptySubscription).data(state).finish()
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/graphql", post_service(GraphQL::new(schema(state))).get(graphiql))
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

/// Reduces a GitHub `resource(url:)` argument — a full URL or just a path
/// — to its path segments.
fn url_segments(url: &str) -> Vec<&str> {
    let path = match url.split_once("://") {
        Some((_, rest)) => rest.find('/').map_or("", |i| &rest[i..]),
        None => url,
    };
    let path = path.split(['?', '#']).next().unwrap_or("");
    path.split('/').filter(|s| !s.is_empty()).collect()
}

/// One query's view of the accounts directory, shared by every object it
/// returns.
struct Snapshot {
    dir: Directory,
    auth: Arc<dyn Authorizer>,
}

impl Snapshot {
    async fn load(ctx: &Context<'_>) -> Result<Arc<Snapshot>> {
        let state = ctx.data::<AppState>()?;
        let dir = state.accounts.load().await.map_err(gql_err)?;
        Ok(Arc::new(Snapshot { dir, auth: state.auth.clone() }))
    }

    fn repository(self: &Arc<Self>, id: Id) -> Option<Repository> {
        let path = self.dir.repo_path(id)?;
        if !self.auth.allow_pull(&path) {
            return None;
        }
        Some(Repository { snap: self.clone(), id, path })
    }

    fn owner(self: &Arc<Self>, id: Id) -> Option<RepositoryOwner> {
        let a = self.dir.account(id)?;
        let snap = self.clone();
        Some(if a.is_org() {
            RepositoryOwner::Organization(Organization { snap, id })
        } else {
            RepositoryOwner::User(User { snap, id })
        })
    }

    fn repositories(self: &Arc<Self>, repos: impl Iterator<Item = Id>, first: Option<i32>) -> RepositoryConnection {
        let mut nodes: Vec<Repository> = repos.filter_map(|id| self.repository(id)).collect();
        nodes.sort_by(|a, b| a.path.cmp(&b.path));
        connection(nodes, first, |nodes, total_count| RepositoryConnection { nodes, total_count })
    }
}

fn connection<T, C>(mut nodes: Vec<T>, first: Option<i32>, make: impl FnOnce(Vec<T>, i32) -> C) -> C {
    let total = nodes.len() as i32;
    if let Some(n) = first {
        nodes.truncate(n.max(0) as usize);
    }
    make(nodes, total)
}

pub struct Query;

#[Object]
impl Query {
    async fn repository(&self, ctx: &Context<'_>, owner: String, name: String) -> Result<Option<Repository>> {
        let snap = Snapshot::load(ctx).await?;
        Ok(snap.dir.resolve_repo(&format!("{owner}/{name}")).and_then(|id| snap.repository(id)))
    }

    async fn repository_owner(&self, ctx: &Context<'_>, login: String) -> Result<Option<RepositoryOwner>> {
        let snap = Snapshot::load(ctx).await?;
        Ok(snap.dir.resolve(&login).and_then(|id| snap.owner(id)))
    }

    async fn user(&self, ctx: &Context<'_>, login: String) -> Result<Option<User>> {
        Ok(match self.repository_owner(ctx, login).await? {
            Some(RepositoryOwner::User(u)) => Some(u),
            _ => None,
        })
    }

    async fn organization(&self, ctx: &Context<'_>, login: String) -> Result<Option<Organization>> {
        Ok(match self.repository_owner(ctx, login).await? {
            Some(RepositoryOwner::Organization(o)) => Some(o),
            _ => None,
        })
    }

    /// Resolves a URL or path to the user, org, or repo it names. Walks
    /// owner segments until one names a repo, then stops — so trailing
    /// segments (`/tree/main/src`) still resolve to the repo, and the
    /// caller can compare `resourcePath` to see what's left over.
    /// Unambiguous because a sub-org and a repo can't share a name
    /// within the same parent.
    async fn resource(&self, ctx: &Context<'_>, url: String) -> Result<Option<UniformResourceLocatable>> {
        let snap = Snapshot::load(ctx).await?;
        let segs = url_segments(&url);
        if segs.is_empty() {
            return Ok(None);
        }
        for i in 0..segs.len() {
            let prefix = segs[..=i].join("/");
            if snap.dir.resolve(&prefix).is_some() {
                continue;
            }
            return Ok(snap
                .dir
                .resolve_repo(&prefix)
                .and_then(|id| snap.repository(id))
                .map(UniformResourceLocatable::Repository));
        }
        let owner = snap.dir.resolve(&segs.join("/")).and_then(|id| snap.owner(id));
        Ok(owner.map(|o| match o {
            RepositoryOwner::User(u) => UniformResourceLocatable::User(u),
            RepositoryOwner::Organization(o) => UniformResourceLocatable::Organization(o),
        }))
    }

    /// Every repo, across all owners (nugitea extension).
    async fn repositories(&self, ctx: &Context<'_>, first: Option<i32>) -> Result<RepositoryConnection> {
        let snap = Snapshot::load(ctx).await?;
        Ok(snap.repositories(snap.dir.repos().map(|r| r.id), first))
    }
}

// clippy reads the macro's repeated `ty = "String"` as a duplicated
// attribute; they're different fields.
#[allow(clippy::duplicated_attributes)]
#[derive(Interface)]
#[graphql(
    field(name = "id", ty = "ID"),
    field(name = "login", ty = "String"),
    field(name = "resource_path", ty = "String"),
    field(name = "repositories", ty = "RepositoryConnection", arg(name = "first", ty = "Option<i32>"))
)]
pub enum RepositoryOwner {
    User(User),
    Organization(Organization),
}

#[derive(Interface)]
#[graphql(field(name = "resource_path", ty = "String"))]
pub enum UniformResourceLocatable {
    Repository(Repository),
    User(User),
    Organization(Organization),
}

#[derive(SimpleObject)]
pub struct RepositoryConnection {
    nodes: Vec<Repository>,
    total_count: i32,
}

#[derive(SimpleObject)]
pub struct OrganizationConnection {
    nodes: Vec<Organization>,
    total_count: i32,
}

#[derive(Enum, Copy, Clone, PartialEq, Eq)]
pub enum RepositoryVisibility {
    Public,
    Internal,
    Private,
}

impl From<Visibility> for RepositoryVisibility {
    fn from(v: Visibility) -> Self {
        match v {
            Visibility::Public => RepositoryVisibility::Public,
            Visibility::Internal => RepositoryVisibility::Internal,
            Visibility::Private => RepositoryVisibility::Private,
        }
    }
}

pub struct User {
    snap: Arc<Snapshot>,
    id: Id,
}

#[Object]
impl User {
    async fn id(&self) -> ID {
        ID(format!("U_{}", self.id))
    }

    async fn login(&self) -> String {
        self.snap.dir.path(self.id).unwrap_or_default()
    }

    async fn resource_path(&self) -> String {
        format!("/{}", self.snap.dir.path(self.id).unwrap_or_default())
    }

    async fn repositories(&self, first: Option<i32>) -> RepositoryConnection {
        self.snap.repositories(self.snap.dir.repos_owned_by(self.id).map(|r| r.id), first)
    }
}

pub struct Organization {
    snap: Arc<Snapshot>,
    id: Id,
}

#[Object]
impl Organization {
    async fn id(&self) -> ID {
        ID(format!("O_{}", self.id))
    }

    /// The org's full path — just its slug for a top-level org, like
    /// GitHub; `parent/child` for a sub-org.
    async fn login(&self) -> String {
        self.snap.dir.path(self.id).unwrap_or_default()
    }

    async fn resource_path(&self) -> String {
        format!("/{}", self.snap.dir.path(self.id).unwrap_or_default())
    }

    async fn repositories(&self, first: Option<i32>) -> RepositoryConnection {
        self.snap.repositories(self.snap.dir.repos_owned_by(self.id).map(|r| r.id), first)
    }

    /// nugitea extension: the org this one is nested under, if any.
    async fn parent_organization(&self) -> Option<Organization> {
        let parent = self.snap.dir.account(self.id)?.parent()?;
        Some(Organization { snap: self.snap.clone(), id: parent })
    }

    /// nugitea extension: orgs nested directly under this one.
    async fn sub_organizations(&self, first: Option<i32>) -> OrganizationConnection {
        let mut orgs: Vec<_> = self.snap.dir.child_orgs(self.id).map(|a| (a.slug.clone(), a.id)).collect();
        orgs.sort();
        let nodes = orgs.into_iter().map(|(_, id)| Organization { snap: self.snap.clone(), id }).collect();
        connection(nodes, first, |nodes, total_count| OrganizationConnection { nodes, total_count })
    }
}

pub struct Repository {
    snap: Arc<Snapshot>,
    id: Id,
    /// Canonical `owner[/sub-org...]/name`, which is also the storage
    /// tier's name for it.
    path: String,
}

impl Repository {
    fn visibility_value(&self) -> Visibility {
        self.snap.dir.repo(self.id).map_or(Visibility::Private, |r| r.visibility)
    }
}

#[Object]
impl Repository {
    async fn id(&self) -> ID {
        ID(format!("R_{}", self.id))
    }

    async fn name(&self) -> &str {
        self.path.rsplit_once('/').map_or(&self.path, |(_, name)| name)
    }

    async fn name_with_owner(&self) -> &str {
        &self.path
    }

    async fn owner(&self) -> Result<RepositoryOwner> {
        let owner = self.snap.dir.repo(self.id).map(|r| r.owner);
        owner.and_then(|o| self.snap.owner(o)).ok_or_else(|| gql_err("repository has no owner"))
    }

    async fn resource_path(&self) -> String {
        format!("/{}", self.path)
    }

    async fn visibility(&self) -> RepositoryVisibility {
        self.visibility_value().into()
    }

    async fn is_private(&self) -> bool {
        self.visibility_value() == Visibility::Private
    }

    /// `main` if it exists, else `master`, else whichever branch sorts
    /// first, else `null` — since HEAD in a freshly created bare repo
    /// doesn't reliably reflect what was actually pushed.
    async fn default_branch_ref(&self, ctx: &Context<'_>) -> Result<Option<Ref>> {
        let state = ctx.data::<AppState>()?;
        let branches = state.storage.branches(&self.path).await.map_err(gql_err)?;
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
        Ok(state.storage.branches(&self.path).await.map_err(gql_err)?.into_iter().map(|name| Ref { name }).collect())
    }

    /// Full recursive file list for the search feature — see the module
    /// doc comment for why this isn't literal GitHub API shape.
    async fn files(&self, ctx: &Context<'_>, r#ref: String) -> Result<Vec<String>> {
        let state = ctx.data::<AppState>()?;
        state.storage.all_files(&self.path, &r#ref).await.map_err(gql_err)
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

        if let Ok(entries) = state.storage.tree(&self.path, git_ref, path).await {
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
        match state.storage.blob(&self.path, git_ref, path).await {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts;
    use crate::auth::AllowAll;
    use crate::storage_client::StorageClient;

    /// A schema over a real accounts.json in a temp dir. The storage tier
    /// isn't running, so only owner-side fields are queryable.
    async fn schema_with(setup: impl FnOnce(&mut Directory) -> anyhow::Result<()>) -> NugiteaSchema {
        let dir =
            std::env::temp_dir().join(format!("nugitea-graphql-test-{}-{}", std::process::id(), rand::random::<u64>()));
        let accounts = Arc::new(accounts::Store::new(&dir).unwrap());
        accounts.update(setup).await.unwrap();
        schema(AppState {
            storage: Arc::new(StorageClient::new("http://127.0.0.1:1".into(), "127.0.0.1:1".into())),
            auth: Arc::new(AllowAll),
            accounts,
        })
    }

    async fn run(schema: &NugiteaSchema, query: &str) -> serde_json::Value {
        let resp = schema.execute(query).await;
        assert!(resp.errors.is_empty(), "{:?}", resp.errors);
        serde_json::to_value(resp.data).unwrap()
    }

    async fn fixture() -> NugiteaSchema {
        schema_with(|d| {
            d.add_user("alice")?;
            d.add_repo_at("alice/dotfiles", Visibility::Private)?;
            d.add_org_at("acme", Visibility::Public)?;
            d.add_org_at("acme/platform", Visibility::Public)?;
            d.add_repo_at("acme/site", Visibility::Public)?;
            d.add_repo_at("acme/platform/api", Visibility::Internal)?;
            Ok(())
        })
        .await
    }

    #[tokio::test]
    async fn repository_by_owner_and_name() {
        let s = fixture().await;
        let v = run(
            &s,
            r#"{ repository(owner: "ACME/platform", name: "api") {
            name nameWithOwner resourcePath visibility isPrivate
            owner { __typename login ... on Organization { parentOrganization { login } } }
        } }"#,
        )
        .await;
        assert_eq!(
            v["repository"],
            serde_json::json!({
                "name": "api", "nameWithOwner": "acme/platform/api", "resourcePath": "/acme/platform/api",
                "visibility": "INTERNAL", "isPrivate": false,
                "owner": { "__typename": "Organization", "login": "acme/platform", "parentOrganization": { "login": "acme" } }
            })
        );
        let v = run(&s, r#"{ repository(owner: "acme", name: "api") { name } }"#).await;
        assert!(v["repository"].is_null());
    }

    #[tokio::test]
    async fn owners_and_connections() {
        let s = fixture().await;
        let v = run(
            &s,
            r#"{
            user(login: "alice") { login repositories(first: 10) { totalCount nodes { name } } }
            notAnOrg: organization(login: "alice") { login }
            organization(login: "acme") {
                repositories { nodes { nameWithOwner } }
                subOrganizations { totalCount nodes { login } }
            }
            repositoryOwner(login: "acme/platform") { __typename resourcePath }
            repositories(first: 2) { totalCount nodes { nameWithOwner } }
        }"#,
        )
        .await;
        assert_eq!(
            v["user"]["repositories"],
            serde_json::json!({ "totalCount": 1, "nodes": [{ "name": "dotfiles" }] })
        );
        assert!(v["notAnOrg"].is_null());
        assert_eq!(v["organization"]["repositories"]["nodes"], serde_json::json!([{ "nameWithOwner": "acme/site" }]));
        assert_eq!(v["organization"]["subOrganizations"]["nodes"], serde_json::json!([{ "login": "acme/platform" }]));
        assert_eq!(
            v["repositoryOwner"],
            serde_json::json!({ "__typename": "Organization", "resourcePath": "/acme/platform" })
        );
        assert_eq!(v["repositories"]["totalCount"], 3);
        assert_eq!(v["repositories"]["nodes"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn checked_in_schema_is_current() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/web/schema.graphql");
        let checked_in = std::fs::read_to_string(path).unwrap_or_default();
        assert!(
            checked_in == sdl(),
            "web/schema.graphql is stale; regenerate it with `cargo run --bin nugitea -- graphql-schema > web/schema.graphql`"
        );
    }

    #[tokio::test]
    async fn resource_by_url() {
        let s = fixture().await;
        let q = |url: &str| format!(r#"{{ resource(url: "{url}") {{ __typename resourcePath }} }}"#);
        let cases = [
            ("/acme", Some(("Organization", "/acme"))),
            ("https://example.com/acme/platform", Some(("Organization", "/acme/platform"))),
            ("/acme/platform/api", Some(("Repository", "/acme/platform/api"))),
            ("/acme/platform/api/tree/main/src?x=1", Some(("Repository", "/acme/platform/api"))),
            ("/Alice/dotfiles/blob/main/.vimrc", Some(("Repository", "/alice/dotfiles"))),
            ("/alice", Some(("User", "/alice"))),
            ("/acme/nope", None),
            ("/", None),
        ];
        for (url, want) in cases {
            let v = run(&s, &q(url)).await;
            match want {
                Some((ty, path)) => {
                    assert_eq!(v["resource"], serde_json::json!({ "__typename": ty, "resourcePath": path }), "{url}")
                }
                None => assert!(v["resource"].is_null(), "{url}"),
            }
        }
    }
}
