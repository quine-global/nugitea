//! Users, orgs, teams, and enterprises — the smallest model that can
//! express how Gitea, GitHub, and GitLab each arrange them. Models only:
//! nothing here is wired into the transports or storage tier yet (repos
//! are still a flat namespace there); this is the shape the `Authorizer`
//! seam will eventually consult.
//!
//! How the three compare:
//!
//! - **Gitea**: users and orgs are the same `user` table with a `type`
//!   column, so they share one slug namespace. Repos are `owner/name`.
//!   Orgs have teams with an access mode (read/write/admin/owner). No org
//!   nesting, no enterprise.
//! - **GitHub**: users and orgs share one login namespace (`RepositoryOwner`
//!   in GraphQL). Repos are `owner/name`. Orgs have teams; repo roles are
//!   read/triage/write/maintain/admin. Enterprises sit above orgs in a
//!   *separate* slug namespace and never own repos; Enterprise Managed
//!   Users are user accounts owned by an enterprise.
//! - **GitLab**: users and groups share the `namespaces` table. Groups nest
//!   arbitrarily (`a/b/c/repo`), and subgroups and projects share one path
//!   space within their parent. Access levels (guest..owner) are inherited
//!   downward. No teams — subgroups and group sharing fill that role. No
//!   enterprise object; the top-level group (or the instance) plays it.
//!
//! The common shape, which is all this module models:
//!
//! - One `Account` namespace for users and orgs. An org may have a parent
//!   org; GitHub and Gitea are just the depth-1 case of GitLab's tree.
//!   Slugs are unique, case-insensitively, among siblings — users and
//!   top-level orgs are siblings under the root, like in all three.
//! - `Enterprise` is a separate namespace grouping top-level orgs (and
//!   optionally managing users), never a repo owner — GitHub's shape.
//!   Gitea just never creates one.
//! - `Team` is a flat, per-org set of users (GitHub/Gitea); GitLab data
//!   maps onto subgroups instead.
//! - Access is one list of `Grant`s: a user or team gets a `Role` on an
//!   org or repo. Org grants flow down to every repo and sub-org beneath
//!   it, and a user's effective role is the max of everything that
//!   applies — additive, like all three. `Role` is GitHub's five-step
//!   ladder, which the other two map onto (see `Role::from_*`).
//! - `Visibility` is public/internal/private everywhere, and a child is
//!   never more visible than its parent (GitLab and Gitea enforce this;
//!   GitHub orgs are always public, so it's a no-op there).
//!
//! Deliberately left out: nested teams, GitHub's per-org base permission
//! (model it as a `Read` grant on the org), custom roles, and GitHub's
//! "internal means enterprise members only" — `Internal` here means any
//! signed-in user, the GitLab/Gitea meaning.

use std::collections::BTreeMap;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::names;

/// Ids are allocated from one counter across every kind, so they're
/// unique globally and never reused. They stay stable across renames,
/// which all three platforms support.
pub type Id = u64;

/// First path segments claimed by app-tier and web routes, which a
/// top-level account can't take once owners occupy the first segment.
pub const RESERVED_SLUGS: &[&str] = &["api", "enterprises", "graphql", "login", "repos", "settings"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Read,
    Triage,
    Write,
    Maintain,
    Admin,
}

impl Role {
    /// GitHub repo permission names, including the REST API's legacy
    /// `pull`/`push` aliases.
    pub fn from_github(permission: &str) -> Option<Role> {
        match permission {
            "read" | "pull" => Some(Role::Read),
            "triage" => Some(Role::Triage),
            "write" | "push" => Some(Role::Write),
            "maintain" => Some(Role::Maintain),
            "admin" => Some(Role::Admin),
            _ => None,
        }
    }

    /// GitLab access levels. Below Reporter (Guest, Planner, Minimal
    /// Access) can't read code in a private project, and nugitea is only
    /// code, so those map to no role at all.
    pub fn from_gitlab(access_level: u32) -> Option<Role> {
        match access_level {
            0..=19 => None,
            20..=29 => Some(Role::Read),
            30..=39 => Some(Role::Write),
            40..=49 => Some(Role::Maintain),
            _ => Some(Role::Admin),
        }
    }

    /// Gitea `AccessMode` values (none=0, read, write, admin, owner=4).
    pub fn from_gitea(access_mode: u8) -> Option<Role> {
        match access_mode {
            1 => Some(Role::Read),
            2 => Some(Role::Write),
            3 | 4 => Some(Role::Admin),
            _ => None,
        }
    }
}

/// Ordered from most to least restricted, so `a <= b` reads as "a is no
/// more visible than b".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Private,
    Internal,
    Public,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum AccountKind {
    User {
        /// The enterprise that owns this account, if any (GitHub EMU,
        /// GitLab enterprise users).
        managed_by: Option<Id>,
    },
    Org {
        /// GitLab subgroups; always `None` for GitHub and Gitea.
        parent: Option<Id>,
        /// Only set on top-level orgs — sub-orgs inherit their root's.
        enterprise: Option<Id>,
    },
}

/// A repo owner: a user or an org, sharing one slug namespace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: Id,
    pub slug: String,
    pub visibility: Visibility,
    #[serde(flatten)]
    pub kind: AccountKind,
}

impl Account {
    fn parent(&self) -> Option<Id> {
        match self.kind {
            AccountKind::Org { parent, .. } => parent,
            AccountKind::User { .. } => None,
        }
    }

    fn is_org(&self) -> bool {
        matches!(self.kind, AccountKind::Org { .. })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Enterprise {
    pub id: Id,
    pub slug: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Team {
    pub id: Id,
    pub org: Id,
    pub slug: String,
    pub members: Vec<Id>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repo {
    pub id: Id,
    pub owner: Id,
    pub slug: String,
    pub visibility: Visibility,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "lowercase")]
pub enum Principal {
    User(Id),
    Team(Id),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "lowercase")]
pub enum Resource {
    Org(Id),
    Repo(Id),
    /// Enterprise administration only — like GitHub, an enterprise grant
    /// never implies access to its orgs' repos.
    Enterprise(Id),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Grant {
    pub principal: Principal,
    pub resource: Resource,
    pub role: Role,
}

/// Every account, team, enterprise, repo, and grant, with the invariants
/// above enforced on insert. Serializable as a whole, so it can persist
/// as one JSON file the way `mirror::Store` does.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Directory {
    next_id: Id,
    accounts: BTreeMap<Id, Account>,
    enterprises: BTreeMap<Id, Enterprise>,
    teams: BTreeMap<Id, Team>,
    repos: BTreeMap<Id, Repo>,
    grants: Vec<Grant>,
}

impl Directory {
    fn alloc(&mut self) -> Id {
        self.next_id += 1;
        self.next_id
    }

    pub fn account(&self, id: Id) -> Option<&Account> {
        self.accounts.get(&id)
    }

    pub fn repo(&self, id: Id) -> Option<&Repo> {
        self.repos.get(&id)
    }

    fn org(&self, id: Id) -> Result<&Account> {
        match self.accounts.get(&id) {
            Some(a) if a.is_org() => Ok(a),
            _ => bail!("no org with id {id}"),
        }
    }

    fn user(&self, id: Id) -> Result<&Account> {
        match self.accounts.get(&id) {
            Some(a) if !a.is_org() => Ok(a),
            _ => bail!("no user with id {id}"),
        }
    }

    /// Checks slug syntax and that nothing under `parent` (None = the
    /// root) already uses it — child orgs and repos share one path space,
    /// as in GitLab.
    fn check_slug(&self, parent: Option<Id>, slug: &str) -> Result<()> {
        names::validate(slug)?;
        if slug.to_ascii_lowercase().ends_with(".git") {
            bail!("slug {slug:?} can't end in .git");
        }
        if parent.is_none() && RESERVED_SLUGS.iter().any(|r| r.eq_ignore_ascii_case(slug)) {
            bail!("slug {slug:?} is reserved");
        }
        let taken = self.accounts.values().any(|a| a.parent() == parent && a.slug.eq_ignore_ascii_case(slug))
            || parent.is_some_and(|p| self.repos.values().any(|r| r.owner == p && r.slug.eq_ignore_ascii_case(slug)));
        if taken {
            bail!("slug {slug:?} is already taken");
        }
        Ok(())
    }

    pub fn add_user(&mut self, slug: &str) -> Result<Id> {
        self.check_slug(None, slug)?;
        let id = self.alloc();
        self.accounts.insert(id, Account {
            id,
            slug: slug.to_string(),
            visibility: Visibility::Public,
            kind: AccountKind::User { managed_by: None },
        });
        Ok(id)
    }

    pub fn add_org(&mut self, slug: &str, parent: Option<Id>, visibility: Visibility) -> Result<Id> {
        if let Some(p) = parent {
            let p = self.org(p).context("parent must be an org")?;
            if visibility > p.visibility {
                bail!("org {slug:?} can't be more visible than its parent");
            }
        }
        self.check_slug(parent, slug)?;
        let id = self.alloc();
        self.accounts.insert(id, Account {
            id,
            slug: slug.to_string(),
            visibility,
            kind: AccountKind::Org { parent, enterprise: None },
        });
        Ok(id)
    }

    pub fn add_enterprise(&mut self, slug: &str) -> Result<Id> {
        names::validate(slug)?;
        if self.enterprises.values().any(|e| e.slug.eq_ignore_ascii_case(slug)) {
            bail!("enterprise {slug:?} already exists");
        }
        let id = self.alloc();
        self.enterprises.insert(id, Enterprise { id, slug: slug.to_string() });
        Ok(id)
    }

    /// Attaches a top-level org to an enterprise.
    pub fn set_org_enterprise(&mut self, org: Id, enterprise: Id) -> Result<()> {
        if !self.enterprises.contains_key(&enterprise) {
            bail!("no enterprise with id {enterprise}");
        }
        self.org(org)?;
        match &mut self.accounts.get_mut(&org).unwrap().kind {
            AccountKind::Org { parent: None, enterprise: e } => *e = Some(enterprise),
            _ => bail!("only top-level orgs belong to an enterprise directly"),
        }
        Ok(())
    }

    /// Marks a user as owned by an enterprise (GitHub EMU).
    pub fn set_user_managed_by(&mut self, user: Id, enterprise: Id) -> Result<()> {
        if !self.enterprises.contains_key(&enterprise) {
            bail!("no enterprise with id {enterprise}");
        }
        self.user(user)?;
        if let AccountKind::User { managed_by } = &mut self.accounts.get_mut(&user).unwrap().kind {
            *managed_by = Some(enterprise);
        }
        Ok(())
    }

    /// The enterprise an account falls under: its own for a managed user,
    /// its root org's for an org.
    pub fn enterprise_of(&self, account: Id) -> Option<Id> {
        let root = self.ancestors(account).last()?;
        match root.kind {
            AccountKind::User { managed_by } => managed_by,
            AccountKind::Org { enterprise, .. } => enterprise,
        }
    }

    pub fn add_team(&mut self, org: Id, slug: &str) -> Result<Id> {
        self.org(org)?;
        names::validate(slug)?;
        if self.teams.values().any(|t| t.org == org && t.slug.eq_ignore_ascii_case(slug)) {
            bail!("team {slug:?} already exists in this org");
        }
        let id = self.alloc();
        self.teams.insert(id, Team { id, org, slug: slug.to_string(), members: vec![] });
        Ok(id)
    }

    pub fn add_team_member(&mut self, team: Id, user: Id) -> Result<()> {
        self.user(user)?;
        let team = self.teams.get_mut(&team).with_context(|| format!("no team with id {team}"))?;
        if !team.members.contains(&user) {
            team.members.push(user);
        }
        Ok(())
    }

    pub fn add_repo(&mut self, owner: Id, slug: &str, visibility: Visibility) -> Result<Id> {
        let o = self.accounts.get(&owner).with_context(|| format!("no account with id {owner}"))?;
        if visibility > o.visibility {
            bail!("repo {slug:?} can't be more visible than its owner");
        }
        self.check_slug(Some(owner), slug)?;
        let id = self.alloc();
        self.repos.insert(id, Repo { id, owner, slug: slug.to_string(), visibility });
        Ok(id)
    }

    /// Records a grant, replacing any existing one for the same
    /// principal and resource. Teams can only be granted things inside
    /// their own org's subtree, and enterprise roles only go to users.
    pub fn grant(&mut self, principal: Principal, resource: Resource, role: Role) -> Result<()> {
        let team_org = match principal {
            Principal::User(u) => {
                self.user(u)?;
                None
            }
            Principal::Team(t) => Some(self.teams.get(&t).with_context(|| format!("no team with id {t}"))?.org),
        };
        let within = match resource {
            Resource::Org(o) => {
                self.org(o)?;
                o
            }
            Resource::Repo(r) => self.repos.get(&r).with_context(|| format!("no repo with id {r}"))?.owner,
            Resource::Enterprise(e) => {
                if !self.enterprises.contains_key(&e) {
                    bail!("no enterprise with id {e}");
                }
                if team_org.is_some() {
                    bail!("enterprise roles can only be granted to users");
                }
                e
            }
        };
        if let Some(org) = team_org {
            if !self.ancestors(within).any(|a| a.id == org) {
                bail!("a team can only be granted access within its own org");
            }
        }
        self.grants.retain(|g| !(g.principal == principal && g.resource == resource));
        self.grants.push(Grant { principal, resource, role });
        Ok(())
    }

    /// The account itself, then each parent org up to the root.
    fn ancestors(&self, account: Id) -> impl Iterator<Item = &Account> {
        std::iter::successors(self.accounts.get(&account), |a| a.parent().and_then(|p| self.accounts.get(&p)))
    }

    /// `a/b/c` for a sub-org, `alice` for a user.
    pub fn path(&self, account: Id) -> Option<String> {
        let mut segs: Vec<&str> = self.ancestors(account).map(|a| a.slug.as_str()).collect();
        if segs.is_empty() {
            return None;
        }
        segs.reverse();
        Some(segs.join("/"))
    }

    pub fn repo_path(&self, repo: Id) -> Option<String> {
        let r = self.repos.get(&repo)?;
        Some(format!("{}/{}", self.path(r.owner)?, r.slug))
    }

    /// Resolves an account path, case-insensitively.
    pub fn resolve(&self, path: &str) -> Option<Id> {
        let mut parent = None;
        for seg in path.split('/') {
            let a = self
                .accounts
                .values()
                .find(|a| a.parent() == parent && a.slug.eq_ignore_ascii_case(seg))?;
            parent = Some(a.id);
        }
        parent
    }

    /// Resolves `owner/path/repo`, case-insensitively.
    pub fn resolve_repo(&self, path: &str) -> Option<Id> {
        let (owner, slug) = path.rsplit_once('/')?;
        let owner = self.resolve(owner)?;
        self.repos
            .values()
            .find(|r| r.owner == owner && r.slug.eq_ignore_ascii_case(slug))
            .map(|r| r.id)
    }

    /// What `user` (None = anonymous) can do to `repo`: the max of the
    /// visibility floor, ownership, and every grant to the user or their
    /// teams on the repo or any org above it. None means no access.
    pub fn effective_role(&self, user: Option<Id>, repo: Id) -> Option<Role> {
        let r = self.repos.get(&repo)?;
        let mut best = match r.visibility {
            Visibility::Public => Some(Role::Read),
            Visibility::Internal if user.is_some() => Some(Role::Read),
            _ => None,
        };
        let Some(user) = user else { return best };
        if r.owner == user {
            return Some(Role::Admin);
        }

        let covers = |res: &Resource| match *res {
            Resource::Repo(id) => id == repo,
            Resource::Org(id) => self.ancestors(r.owner).any(|a| a.id == id),
            Resource::Enterprise(_) => false,
        };
        let holds = |p: &Principal| match *p {
            Principal::User(id) => id == user,
            Principal::Team(id) => self.teams.get(&id).is_some_and(|t| t.members.contains(&user)),
        };
        for g in &self.grants {
            if holds(&g.principal) && covers(&g.resource) {
                best = best.max(Some(g.role));
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// enterprise acme-corp
    ///   acme (org, internal)
    ///     platform (sub-org, private)
    ///       api (repo, private)
    ///     site (repo, internal)
    /// alice, bob, carol (users); team acme/devs = [bob]
    struct Fixture {
        dir: Directory,
        ent: Id,
        acme: Id,
        platform: Id,
        api: Id,
        site: Id,
        alice: Id,
        bob: Id,
        carol: Id,
        devs: Id,
    }

    fn fixture() -> Fixture {
        let mut dir = Directory::default();
        let ent = dir.add_enterprise("acme-corp").unwrap();
        let acme = dir.add_org("acme", None, Visibility::Internal).unwrap();
        dir.set_org_enterprise(acme, ent).unwrap();
        let platform = dir.add_org("platform", Some(acme), Visibility::Private).unwrap();
        let api = dir.add_repo(platform, "api", Visibility::Private).unwrap();
        let site = dir.add_repo(acme, "site", Visibility::Internal).unwrap();
        let alice = dir.add_user("alice").unwrap();
        let bob = dir.add_user("bob").unwrap();
        let carol = dir.add_user("carol").unwrap();
        let devs = dir.add_team(acme, "devs").unwrap();
        dir.add_team_member(devs, bob).unwrap();
        Fixture { dir, ent, acme, platform, api, site, alice, bob, carol, devs }
    }

    #[test]
    fn users_and_top_level_orgs_share_a_case_insensitive_namespace() {
        let mut f = fixture();
        assert!(f.dir.add_user("ACME").is_err());
        assert!(f.dir.add_org("Alice", None, Visibility::Public).is_err());
        // but a sub-org is scoped to its parent
        assert!(f.dir.add_org("alice", Some(f.acme), Visibility::Private).is_ok());
    }

    #[test]
    fn sub_orgs_and_repos_share_a_path_space() {
        let mut f = fixture();
        assert!(f.dir.add_repo(f.acme, "platform", Visibility::Private).is_err());
        assert!(f.dir.add_org("site", Some(f.acme), Visibility::Private).is_err());
    }

    #[test]
    fn rejects_reserved_and_malformed_slugs() {
        let mut dir = Directory::default();
        assert!(dir.add_user("repos").is_err());
        assert!(dir.add_user("foo.git").is_err());
        assert!(dir.add_user("-foo").is_err());
        assert!(dir.add_user("a/b").is_err());
    }

    #[test]
    fn structural_invariants() {
        let mut f = fixture();
        // users can't parent orgs
        assert!(f.dir.add_org("x", Some(f.alice), Visibility::Private).is_err());
        // children can't be more visible than parents
        assert!(f.dir.add_org("x", Some(f.platform), Visibility::Public).is_err());
        assert!(f.dir.add_repo(f.platform, "x", Visibility::Internal).is_err());
        // only top-level orgs join an enterprise; sub-orgs inherit
        assert!(f.dir.set_org_enterprise(f.platform, f.ent).is_err());
        assert_eq!(f.dir.enterprise_of(f.platform), Some(f.ent));
        assert_eq!(f.dir.enterprise_of(f.alice), None);
        f.dir.set_user_managed_by(f.alice, f.ent).unwrap();
        assert_eq!(f.dir.enterprise_of(f.alice), Some(f.ent));
    }

    #[test]
    fn paths_round_trip() {
        let f = fixture();
        assert_eq!(f.dir.path(f.platform).as_deref(), Some("acme/platform"));
        assert_eq!(f.dir.repo_path(f.api).as_deref(), Some("acme/platform/api"));
        assert_eq!(f.dir.resolve("ACME/Platform"), Some(f.platform));
        assert_eq!(f.dir.resolve_repo("acme/platform/api"), Some(f.api));
        assert_eq!(f.dir.resolve_repo("acme/api"), None);
    }

    #[test]
    fn visibility_floor() {
        let f = fixture();
        assert_eq!(f.dir.effective_role(None, f.site), None);
        assert_eq!(f.dir.effective_role(Some(f.carol), f.site), Some(Role::Read));
        assert_eq!(f.dir.effective_role(Some(f.carol), f.api), None);
    }

    #[test]
    fn grants_inherit_down_the_tree_and_take_the_max() {
        let mut f = fixture();
        f.dir.grant(Principal::User(f.alice), Resource::Org(f.acme), Role::Read).unwrap();
        f.dir.grant(Principal::User(f.alice), Resource::Org(f.platform), Role::Maintain).unwrap();
        f.dir.grant(Principal::Team(f.devs), Resource::Repo(f.api), Role::Write).unwrap();
        assert_eq!(f.dir.effective_role(Some(f.alice), f.api), Some(Role::Maintain));
        assert_eq!(f.dir.effective_role(Some(f.alice), f.site), Some(Role::Read));
        assert_eq!(f.dir.effective_role(Some(f.bob), f.api), Some(Role::Write));
        assert_eq!(f.dir.effective_role(Some(f.bob), f.site), Some(Role::Read)); // internal floor
        // re-granting replaces rather than stacks
        f.dir.grant(Principal::User(f.alice), Resource::Org(f.platform), Role::Triage).unwrap();
        assert_eq!(f.dir.effective_role(Some(f.alice), f.api), Some(Role::Triage));
    }

    #[test]
    fn grant_scoping() {
        let mut f = fixture();
        let other = f.dir.add_org("other", None, Visibility::Public).unwrap();
        let theirs = f.dir.add_repo(other, "x", Visibility::Public).unwrap();
        assert!(f.dir.grant(Principal::Team(f.devs), Resource::Repo(theirs), Role::Read).is_err());
        assert!(f.dir.grant(Principal::Team(f.devs), Resource::Enterprise(f.ent), Role::Admin).is_err());
        // enterprise admin grants no repo access
        f.dir.grant(Principal::User(f.carol), Resource::Enterprise(f.ent), Role::Admin).unwrap();
        assert_eq!(f.dir.effective_role(Some(f.carol), f.api), None);
    }

    #[test]
    fn user_owns_their_repos() {
        let mut f = fixture();
        let dots = f.dir.add_repo(f.alice, "dotfiles", Visibility::Private).unwrap();
        assert_eq!(f.dir.effective_role(Some(f.alice), dots), Some(Role::Admin));
        assert_eq!(f.dir.effective_role(Some(f.bob), dots), None);
        assert_eq!(f.dir.repo_path(dots).as_deref(), Some("alice/dotfiles"));
    }

    #[test]
    fn role_mappings() {
        assert_eq!(Role::from_github("push"), Some(Role::Write));
        assert_eq!(Role::from_gitlab(10), None); // guest
        assert_eq!(Role::from_gitlab(20), Some(Role::Read));
        assert_eq!(Role::from_gitlab(50), Some(Role::Admin));
        assert_eq!(Role::from_gitea(4), Some(Role::Admin));
        assert_eq!(Role::from_gitea(0), None);
    }

    #[test]
    fn serde_round_trip() {
        let f = fixture();
        let json = serde_json::to_string(&f.dir).unwrap();
        let back: Directory = serde_json::from_str(&json).unwrap();
        assert_eq!(back.repo_path(f.api), f.dir.repo_path(f.api));
    }
}
