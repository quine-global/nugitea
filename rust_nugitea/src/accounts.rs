//! Users, orgs, teams, and enterprises — one model that can express how
//! Gitea, GitHub, and GitLab each arrange them, covering GitHub's full
//! account/org/enterprise feature set. Models only: nothing here is wired
//! into the transports or storage tier yet (repos are still a flat
//! namespace there); this is the shape the `Authorizer` seam will
//! eventually consult.
//!
//! How the three compare:
//!
//! - **Gitea**: users and orgs are the same `user` table with a `type`
//!   column, so they share one slug namespace. Repos are `owner/name`.
//!   Orgs have teams with an access mode (read/write/admin/owner). No org
//!   nesting, no enterprise.
//! - **GitHub**: users and orgs share one login namespace (`RepositoryOwner`
//!   in GraphQL). Repos are `owner/name`. Orgs have owners, members, and
//!   billing managers, a base permission for all members, nested teams,
//!   custom repo roles, and outside collaborators; repo roles are
//!   read/triage/write/maintain/admin. Enterprises sit above orgs in a
//!   *separate* slug namespace and never own repos; Enterprise Managed
//!   Users are user accounts owned by an enterprise and confined to it.
//! - **GitLab**: users and groups share the `namespaces` table. Groups nest
//!   arbitrarily (`a/b/c/repo`), and subgroups and projects share one path
//!   space within their parent. Access levels (guest..owner) are inherited
//!   downward. No teams — subgroups and group sharing fill that role. No
//!   enterprise object; the top-level group (or the instance) plays it.
//!
//! The model:
//!
//! - One `Account` namespace for users and orgs. An org may have a parent
//!   org; GitHub and Gitea are just the depth-1 case of GitLab's tree.
//!   Slugs are unique, case-insensitively, among siblings — users and
//!   top-level orgs are siblings under the root, like in all three.
//! - Org membership is explicit (`OrgRole`) and inherited down the tree,
//!   GitLab-style. Owners get admin on every repo beneath the org; members
//!   get the org's `base_role`; billing managers aren't members at all.
//!   Anyone holding only repo grants is an outside collaborator.
//! - `Team`s belong to one org and may nest (GitHub): a child team's
//!   members inherit the parent team's grants. Secret teams can't nest.
//!   Team members must be org members.
//! - Access is one list of `Grant`s: a user or team gets a built-in `Role`
//!   or an org-defined `CustomRole` (a base role plus extra named
//!   permissions) on an org or repo. Org grants flow down to every repo
//!   and sub-org beneath it — GitHub's "all-repo-*" org roles are exactly
//!   this. A user's effective access is the max of everything that
//!   applies, additive like all three. `Role` is GitHub's five-step
//!   ladder, which the other two map onto (see `Role::from_*`).
//! - `Enterprise` is a separate namespace grouping top-level orgs, never a
//!   repo owner. It has owners and billing managers (`EnterpriseRole`),
//!   which grant no repo access. Its members are its orgs' members plus
//!   its managed users. An enterprise with a `shortcode` is a managed
//!   (EMU) enterprise: its users are named `handle_shortcode`, only they
//!   can join its orgs or collaborate on its repos, they can't join or
//!   contribute outside it, and they can't own public repos. Gitea never
//!   creates an enterprise.
//! - `Visibility` is public/internal/private, and a child is never more
//!   visible than its parent (GitLab and Gitea enforce this; GitHub orgs
//!   are always public, so it's a no-op there). `Internal` means members
//!   of the owning enterprise, if there is one (GitHub), else any
//!   signed-in user (GitLab/Gitea).
//! - An account can `block` a user: they keep only what visibility gives
//!   anyone, and can't be added or granted anything beneath it.
//!
//! Left out, as workflow or features nugitea doesn't have rather than
//! structure: invitations, renames and redirects, forks, bot/app
//! accounts, IdP/SAML team sync, and org roles with no repo effect
//! (moderator, security manager).

use std::collections::{BTreeMap, BTreeSet};

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

/// Ordered so that `>= Member` means "is a member".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrgRole {
    BillingManager,
    Member,
    Owner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnterpriseRole {
    BillingManager,
    Owner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TeamRole {
    Member,
    Maintainer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TeamPrivacy {
    Visible,
    Secret,
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
        /// What every member gets on every repo beneath this org.
        #[serde(default)]
        base_role: Option<Role>,
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
    /// Set for a managed (EMU) enterprise; its users' slugs end in
    /// `_<shortcode>`.
    pub shortcode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Team {
    pub id: Id,
    pub org: Id,
    pub parent: Option<Id>,
    pub slug: String,
    pub privacy: TeamPrivacy,
    pub members: BTreeMap<Id, TeamRole>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repo {
    pub id: Id,
    pub owner: Id,
    pub slug: String,
    pub visibility: Visibility,
}

/// An org-defined repo role: a built-in base plus extra named
/// permissions (GitHub's fine-grained ones, e.g. `"manage_webhooks"`),
/// usable on anything beneath the org.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomRole {
    pub id: Id,
    pub org: Id,
    pub name: String,
    pub base: Role,
    pub permissions: BTreeSet<String>,
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
}

/// A built-in role (serialized as its name) or a custom role's id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GrantRole {
    Builtin(Role),
    Custom(Id),
}

impl From<Role> for GrantRole {
    fn from(r: Role) -> Self {
        GrantRole::Builtin(r)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Grant {
    pub principal: Principal,
    pub resource: Resource,
    pub role: GrantRole,
}

/// What a user can do to a repo: a built-in role (None = no access) plus
/// any extra permissions from custom roles.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Access {
    pub role: Option<Role>,
    pub permissions: BTreeSet<String>,
}

/// Everything above, with its invariants enforced on every change.
/// Serializable as a whole, so it can persist as one JSON file the way
/// `mirror::Store` does.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Directory {
    next_id: Id,
    accounts: BTreeMap<Id, Account>,
    enterprises: BTreeMap<Id, Enterprise>,
    teams: BTreeMap<Id, Team>,
    repos: BTreeMap<Id, Repo>,
    custom_roles: BTreeMap<Id, CustomRole>,
    /// org -> user -> role
    org_members: BTreeMap<Id, BTreeMap<Id, OrgRole>>,
    /// enterprise -> user -> role
    enterprise_roles: BTreeMap<Id, BTreeMap<Id, EnterpriseRole>>,
    /// (blocker account, blocked user)
    blocks: BTreeSet<(Id, Id)>,
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

    pub fn team(&self, id: Id) -> Option<&Team> {
        self.teams.get(&id)
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

    fn enterprise(&self, id: Id) -> Result<&Enterprise> {
        self.enterprises.get(&id).with_context(|| format!("no enterprise with id {id}"))
    }

    // ---- structure --------------------------------------------------

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

    fn insert_user(&mut self, slug: String, managed_by: Option<Id>) -> Id {
        let id = self.alloc();
        self.accounts.insert(id, Account {
            id,
            slug,
            visibility: Visibility::Public,
            kind: AccountKind::User { managed_by },
        });
        id
    }

    pub fn add_user(&mut self, slug: &str) -> Result<Id> {
        self.check_slug(None, slug)?;
        Ok(self.insert_user(slug.to_string(), None))
    }

    /// Creates an EMU account named `<handle>_<shortcode>`.
    pub fn add_managed_user(&mut self, enterprise: Id, handle: &str) -> Result<Id> {
        let shortcode = self
            .enterprise(enterprise)?
            .shortcode
            .clone()
            .context("only a managed enterprise has managed users")?;
        let slug = format!("{handle}_{shortcode}");
        self.check_slug(None, &slug)?;
        Ok(self.insert_user(slug, Some(enterprise)))
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
            kind: AccountKind::Org { parent, enterprise: None, base_role: None },
        });
        Ok(id)
    }

    pub fn set_base_role(&mut self, org: Id, role: Option<Role>) -> Result<()> {
        self.org(org)?;
        if let AccountKind::Org { base_role, .. } = &mut self.accounts.get_mut(&org).unwrap().kind {
            *base_role = role;
        }
        Ok(())
    }

    /// A shortcode makes it a managed (EMU) enterprise.
    pub fn add_enterprise(&mut self, slug: &str, shortcode: Option<&str>) -> Result<Id> {
        names::validate(slug)?;
        if self.enterprises.values().any(|e| e.slug.eq_ignore_ascii_case(slug)) {
            bail!("enterprise {slug:?} already exists");
        }
        if let Some(sc) = shortcode {
            if sc.is_empty() || !sc.chars().all(|c| c.is_ascii_alphanumeric()) {
                bail!("invalid shortcode {sc:?}");
            }
            if self.enterprises.values().any(|e| e.shortcode.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(sc))) {
                bail!("shortcode {sc:?} is already taken");
            }
        }
        let id = self.alloc();
        self.enterprises.insert(id, Enterprise {
            id,
            slug: slug.to_string(),
            shortcode: shortcode.map(str::to_string),
        });
        Ok(id)
    }

    /// Attaches a top-level org to an enterprise, or moves it between
    /// enterprises. Moving into a managed enterprise requires everyone
    /// already involved with the org to be one of its managed users.
    pub fn set_org_enterprise(&mut self, org: Id, enterprise: Option<Id>) -> Result<()> {
        if let Some(e) = enterprise {
            self.enterprise(e)?;
        }
        let prev = match self.org(org)?.kind {
            AccountKind::Org { parent: None, enterprise, .. } => enterprise,
            _ => bail!("only top-level orgs belong to an enterprise directly"),
        };
        self.set_org_enterprise_unchecked(org, enterprise);
        let involved = self.users_involved_in(org);
        if let Some(bad) = involved.into_iter().find(|&u| self.check_enterprise_boundary(org, u).is_err()) {
            self.set_org_enterprise_unchecked(org, prev);
            bail!("user {bad} can't belong to an org in this enterprise");
        }
        Ok(())
    }

    fn set_org_enterprise_unchecked(&mut self, org: Id, to: Option<Id>) {
        if let AccountKind::Org { enterprise, .. } = &mut self.accounts.get_mut(&org).unwrap().kind {
            *enterprise = to;
        }
    }

    pub fn add_team(&mut self, org: Id, slug: &str, parent: Option<Id>, privacy: TeamPrivacy) -> Result<Id> {
        self.org(org)?;
        names::validate(slug)?;
        if self.teams.values().any(|t| t.org == org && t.slug.eq_ignore_ascii_case(slug)) {
            bail!("team {slug:?} already exists in this org");
        }
        if let Some(p) = parent {
            let p = self.teams.get(&p).with_context(|| format!("no team with id {p}"))?;
            if p.org != org {
                bail!("a parent team must be in the same org");
            }
            if p.privacy == TeamPrivacy::Secret || privacy == TeamPrivacy::Secret {
                bail!("secret teams can't be nested");
            }
        }
        let id = self.alloc();
        self.teams.insert(id, Team {
            id,
            org,
            parent,
            slug: slug.to_string(),
            privacy,
            members: BTreeMap::new(),
        });
        Ok(id)
    }

    pub fn add_repo(&mut self, owner: Id, slug: &str, visibility: Visibility) -> Result<Id> {
        let o = self.accounts.get(&owner).with_context(|| format!("no account with id {owner}"))?;
        if visibility > o.visibility {
            bail!("repo {slug:?} can't be more visible than its owner");
        }
        if matches!(o.kind, AccountKind::User { managed_by: Some(_) }) && visibility == Visibility::Public {
            bail!("managed users can't own public repos");
        }
        self.check_slug(Some(owner), slug)?;
        let id = self.alloc();
        self.repos.insert(id, Repo { id, owner, slug: slug.to_string(), visibility });
        Ok(id)
    }

    pub fn add_custom_role(&mut self, org: Id, name: &str, base: Role, permissions: &[&str]) -> Result<Id> {
        self.org(org)?;
        names::validate(name)?;
        let builtin = serde_json::from_value::<Role>(serde_json::Value::String(name.to_ascii_lowercase())).is_ok();
        if builtin || self.custom_roles.values().any(|r| r.org == org && r.name.eq_ignore_ascii_case(name)) {
            bail!("role {name:?} already exists");
        }
        let id = self.alloc();
        self.custom_roles.insert(id, CustomRole {
            id,
            org,
            name: name.to_string(),
            base,
            permissions: permissions.iter().map(|p| p.to_string()).collect(),
        });
        Ok(id)
    }

    // ---- membership -------------------------------------------------

    /// Sets (Some) or removes (None) a user's direct role in an org.
    /// Removal, or demotion to billing manager, also drops everything
    /// the user no longer qualifies for beneath the org — team seats and
    /// every grant, repo grants included (re-grant repos afterwards to
    /// keep them as an outside collaborator). An org that has an owner
    /// can't lose its last one.
    pub fn set_org_member(&mut self, org: Id, user: Id, role: Option<OrgRole>) -> Result<()> {
        self.org(org)?;
        self.user(user)?;
        if role.is_some() {
            self.check_can_touch(org, user)?;
        }
        let members = self.org_members.entry(org).or_default();
        if members.get(&user) == Some(&OrgRole::Owner)
            && role != Some(OrgRole::Owner)
            && members.values().filter(|r| **r == OrgRole::Owner).count() == 1
        {
            bail!("can't remove an org's last owner");
        }
        match role {
            Some(r) => members.insert(user, r),
            None => members.remove(&user),
        };
        if role.is_none_or(|r| r < OrgRole::Member) {
            self.prune_nonmember(org, user);
        }
        Ok(())
    }

    fn prune_nonmember(&mut self, org: Id, user: Id) {
        let orphaned: BTreeSet<Id> = self.subtree(org).into_iter().filter(|&o| !self.is_org_member(o, user)).collect();
        for t in self.teams.values_mut().filter(|t| orphaned.contains(&t.org)) {
            t.members.remove(&user);
        }
        self.drop_user_grants_within(user, &orphaned);
    }

    /// The user's role in an org, direct or inherited from an ancestor.
    pub fn org_role(&self, org: Id, user: Id) -> Option<OrgRole> {
        self.ancestors(org)
            .filter_map(|a| self.org_members.get(&a.id)?.get(&user).copied())
            .max()
    }

    pub fn is_org_member(&self, org: Id, user: Id) -> bool {
        self.org_role(org, user).is_some_and(|r| r >= OrgRole::Member)
    }

    /// Sets (Some) or removes (None) a user's seat on a team. They must
    /// already be a member of the team's org.
    pub fn set_team_member(&mut self, team: Id, user: Id, role: Option<TeamRole>) -> Result<()> {
        self.user(user)?;
        let org = self.teams.get(&team).with_context(|| format!("no team with id {team}"))?.org;
        if role.is_some() && !self.is_org_member(org, user) {
            bail!("team members must be members of the team's org");
        }
        let members = &mut self.teams.get_mut(&team).unwrap().members;
        match role {
            Some(r) => members.insert(user, r),
            None => members.remove(&user),
        };
        Ok(())
    }

    /// Teams the user is on, plus every ancestor of those — a child
    /// team's members inherit the parent team's access.
    fn teams_of(&self, user: Id) -> BTreeSet<Id> {
        let mut out = BTreeSet::new();
        for t in self.teams.values().filter(|t| t.members.contains_key(&user)) {
            let mut cur = Some(t);
            while let Some(t) = cur {
                if !out.insert(t.id) {
                    break;
                }
                cur = t.parent.and_then(|p| self.teams.get(&p));
            }
        }
        out
    }

    /// Sets (Some) or removes (None) an enterprise-level role. These
    /// administer the enterprise; they grant no repo access.
    pub fn set_enterprise_role(&mut self, enterprise: Id, user: Id, role: Option<EnterpriseRole>) -> Result<()> {
        let e = self.enterprise(enterprise)?;
        self.user(user)?;
        if e.shortcode.is_some() && role.is_some() && self.enterprise_of(user) != Some(enterprise) {
            bail!("a managed enterprise's roles can only go to its managed users");
        }
        let roles = self.enterprise_roles.entry(enterprise).or_default();
        match role {
            Some(r) => roles.insert(user, r),
            None => roles.remove(&user),
        };
        Ok(())
    }

    pub fn enterprise_role(&self, enterprise: Id, user: Id) -> Option<EnterpriseRole> {
        self.enterprise_roles.get(&enterprise)?.get(&user).copied()
    }

    /// Its managed users, plus members of any of its orgs.
    pub fn is_enterprise_member(&self, enterprise: Id, user: Id) -> bool {
        self.enterprise_of(user) == Some(enterprise)
            || self.org_members.iter().any(|(&org, members)| {
                self.enterprise_of(org) == Some(enterprise) && members.get(&user).is_some_and(|r| *r >= OrgRole::Member)
            })
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

    // ---- grants and blocks ------------------------------------------

    /// Records a grant, replacing any existing one for the same principal
    /// and resource. Teams and custom roles only reach within their own
    /// org's subtree; org-wide grants to a user require membership (repo
    /// grants don't — that's an outside collaborator).
    pub fn grant(&mut self, principal: Principal, resource: Resource, role: impl Into<GrantRole>) -> Result<()> {
        let role = role.into();
        let within = match resource {
            Resource::Org(o) => self.org(o)?.id,
            Resource::Repo(r) => self.repos.get(&r).with_context(|| format!("no repo with id {r}"))?.owner,
        };
        match principal {
            Principal::User(u) => {
                self.user(u)?;
                self.check_can_touch(within, u)?;
                if let Resource::Org(o) = resource {
                    if !self.is_org_member(o, u) {
                        bail!("org-wide grants to a user require org membership");
                    }
                }
            }
            Principal::Team(t) => {
                let org = self.teams.get(&t).with_context(|| format!("no team with id {t}"))?.org;
                if !self.is_within(within, org) {
                    bail!("a team can only be granted access within its own org");
                }
            }
        }
        if let GrantRole::Custom(c) = role {
            let org = self.custom_roles.get(&c).with_context(|| format!("no custom role with id {c}"))?.org;
            if !self.is_within(within, org) {
                bail!("a custom role can only be used within the org that defines it");
            }
        }
        self.revoke(principal, resource);
        self.grants.push(Grant { principal, resource, role });
        Ok(())
    }

    pub fn revoke(&mut self, principal: Principal, resource: Resource) {
        self.grants.retain(|g| !(g.principal == principal && g.resource == resource));
    }

    /// Blocks a user from everything beneath `blocker` (a user or org),
    /// dropping any grants they held there. An org has to remove a
    /// member before blocking them.
    pub fn block(&mut self, blocker: Id, user: Id) -> Result<()> {
        self.accounts.get(&blocker).with_context(|| format!("no account with id {blocker}"))?;
        self.user(user)?;
        if blocker == user {
            bail!("can't block yourself");
        }
        if self.org_role(blocker, user).is_some() {
            bail!("remove them from the org before blocking them");
        }
        self.blocks.insert((blocker, user));
        let subtree = self.subtree(blocker).into_iter().collect();
        self.drop_user_grants_within(user, &subtree);
        Ok(())
    }

    pub fn unblock(&mut self, blocker: Id, user: Id) {
        self.blocks.remove(&(blocker, user));
    }

    fn is_blocked_within(&self, account: Id, user: Id) -> bool {
        self.ancestors(account).any(|a| self.blocks.contains(&(a.id, user)))
    }

    /// Gates adding a user to anything beneath `account`: not blocked
    /// there, and on the right side of any managed-enterprise boundary.
    fn check_can_touch(&self, account: Id, user: Id) -> Result<()> {
        if self.is_blocked_within(account, user) {
            bail!("user {user} is blocked here");
        }
        self.check_enterprise_boundary(account, user)
    }

    /// Managed users stay inside their enterprise; a managed enterprise
    /// admits only its own managed users.
    fn check_enterprise_boundary(&self, account: Id, user: Id) -> Result<()> {
        let home = self.enterprise_of(user);
        let here = self.enterprise_of(account);
        if home.is_some() && home != here {
            bail!("managed users can't join or collaborate outside their enterprise");
        }
        if let Some(e) = here {
            if self.enterprises[&e].shortcode.is_some() && home != Some(e) {
                bail!("a managed enterprise only admits its own managed users");
            }
        }
        Ok(())
    }

    fn drop_user_grants_within(&mut self, user: Id, accounts: &BTreeSet<Id>) {
        let repos = &self.repos;
        self.grants.retain(|g| {
            let owner = match g.resource {
                Resource::Org(o) => o,
                Resource::Repo(r) => repos[&r].owner,
            };
            !(g.principal == Principal::User(user) && accounts.contains(&owner))
        });
    }

    /// Every user with a membership, team seat, or grant beneath `org`.
    fn users_involved_in(&self, org: Id) -> BTreeSet<Id> {
        let subtree: BTreeSet<Id> = self.subtree(org).into_iter().collect();
        let mut users = BTreeSet::new();
        for o in &subtree {
            users.extend(self.org_members.get(o).into_iter().flat_map(|m| m.keys()));
        }
        for g in &self.grants {
            let owner = match g.resource {
                Resource::Org(o) => o,
                Resource::Repo(r) => self.repos[&r].owner,
            };
            if let (Principal::User(u), true) = (g.principal, subtree.contains(&owner)) {
                users.insert(u);
            }
        }
        users
    }

    // ---- paths ------------------------------------------------------

    /// The account itself, then each parent org up to the root.
    fn ancestors(&self, account: Id) -> impl Iterator<Item = &Account> {
        std::iter::successors(self.accounts.get(&account), |a| a.parent().and_then(|p| self.accounts.get(&p)))
    }

    fn is_within(&self, account: Id, ancestor: Id) -> bool {
        self.ancestors(account).any(|a| a.id == ancestor)
    }

    /// `account` and every account beneath it.
    fn subtree(&self, account: Id) -> Vec<Id> {
        self.accounts.keys().copied().filter(|&a| self.is_within(a, account)).collect()
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

    // ---- access -----------------------------------------------------

    /// What `user` (None = anonymous) can do to `repo`. Starts from the
    /// visibility floor; a blocked user, or a managed user outside their
    /// enterprise, stops there. Otherwise it's the max of: owning the
    /// repo or being an owner of any org above it (admin), the base role
    /// of every such org they're a member of, and every grant to them or
    /// their teams on the repo or any org above it.
    pub fn effective_access(&self, user: Option<Id>, repo: Id) -> Access {
        let Some(r) = self.repos.get(&repo) else { return Access::default() };
        let enterprise = self.enterprise_of(r.owner);
        let floor = match r.visibility {
            Visibility::Public => Some(Role::Read),
            Visibility::Internal => user
                .filter(|&u| enterprise.is_none_or(|e| self.is_enterprise_member(e, u)))
                .map(|_| Role::Read),
            Visibility::Private => None,
        };
        let mut access = Access { role: floor, permissions: BTreeSet::new() };
        let Some(user) = user else { return access };
        let home = self.enterprise_of(user);
        if self.is_blocked_within(r.owner, user) || (home.is_some() && home != enterprise) {
            return access;
        }
        if r.owner == user || self.org_role(r.owner, user) == Some(OrgRole::Owner) {
            access.role = Some(Role::Admin);
            return access;
        }

        for a in self.ancestors(r.owner) {
            if let AccountKind::Org { base_role: Some(base), .. } = a.kind {
                if self.is_org_member(a.id, user) {
                    access.role = access.role.max(Some(base));
                }
            }
        }

        let teams = self.teams_of(user);
        for g in &self.grants {
            let holds = match g.principal {
                Principal::User(id) => id == user,
                Principal::Team(id) => teams.contains(&id),
            };
            let covers = match g.resource {
                Resource::Repo(id) => id == repo,
                Resource::Org(id) => self.is_within(r.owner, id),
            };
            if !(holds && covers) {
                continue;
            }
            let role = match g.role {
                GrantRole::Builtin(role) => role,
                GrantRole::Custom(id) => {
                    let c = &self.custom_roles[&id];
                    access.permissions.extend(c.permissions.iter().cloned());
                    c.base
                }
            };
            access.role = access.role.max(Some(role));
        }
        access
    }

    pub fn effective_role(&self, user: Option<Id>, repo: Id) -> Option<Role> {
        self.effective_access(user, repo).role
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// enterprise acme-corp (unmanaged)
    ///   acme (org, internal)
    ///     platform (sub-org, private)
    ///       api (repo, private)
    ///     site (repo, internal)
    /// alice, bob, carol (users); alice owns acme, bob is a member;
    /// team acme/devs = [bob]
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
        let ent = dir.add_enterprise("acme-corp", None).unwrap();
        let acme = dir.add_org("acme", None, Visibility::Internal).unwrap();
        dir.set_org_enterprise(acme, Some(ent)).unwrap();
        let platform = dir.add_org("platform", Some(acme), Visibility::Private).unwrap();
        let api = dir.add_repo(platform, "api", Visibility::Private).unwrap();
        let site = dir.add_repo(acme, "site", Visibility::Internal).unwrap();
        let alice = dir.add_user("alice").unwrap();
        let bob = dir.add_user("bob").unwrap();
        let carol = dir.add_user("carol").unwrap();
        dir.set_org_member(acme, alice, Some(OrgRole::Owner)).unwrap();
        dir.set_org_member(acme, bob, Some(OrgRole::Member)).unwrap();
        let devs = dir.add_team(acme, "devs", None, TeamPrivacy::Visible).unwrap();
        dir.set_team_member(devs, bob, Some(TeamRole::Member)).unwrap();
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
        assert!(f.dir.set_org_enterprise(f.platform, Some(f.ent)).is_err());
        assert_eq!(f.dir.enterprise_of(f.platform), Some(f.ent));
        assert_eq!(f.dir.enterprise_of(f.alice), None);
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
    fn internal_means_enterprise_members_inside_an_enterprise() {
        let mut f = fixture();
        assert_eq!(f.dir.effective_role(None, f.site), None);
        assert_eq!(f.dir.effective_role(Some(f.carol), f.site), None);
        assert_eq!(f.dir.effective_role(Some(f.bob), f.site), Some(Role::Read));
        assert_eq!(f.dir.effective_role(Some(f.bob), f.api), None);
        // a member of a sibling org in the same enterprise counts too
        let other = f.dir.add_org("other", None, Visibility::Public).unwrap();
        f.dir.set_org_enterprise(other, Some(f.ent)).unwrap();
        f.dir.set_org_member(other, f.carol, Some(OrgRole::Member)).unwrap();
        assert_eq!(f.dir.effective_role(Some(f.carol), f.site), Some(Role::Read));
    }

    #[test]
    fn internal_means_any_signed_in_user_outside_an_enterprise() {
        let mut f = fixture();
        let gl = f.dir.add_org("gl", None, Visibility::Internal).unwrap();
        let repo = f.dir.add_repo(gl, "x", Visibility::Internal).unwrap();
        assert_eq!(f.dir.effective_role(None, repo), None);
        assert_eq!(f.dir.effective_role(Some(f.carol), repo), Some(Role::Read));
    }

    #[test]
    fn org_owners_are_admin_everywhere_beneath() {
        let f = fixture();
        assert_eq!(f.dir.effective_role(Some(f.alice), f.api), Some(Role::Admin));
        assert_eq!(f.dir.org_role(f.platform, f.alice), Some(OrgRole::Owner));
    }

    #[test]
    fn base_role_applies_to_members_only() {
        let mut f = fixture();
        f.dir.set_base_role(f.acme, Some(Role::Write)).unwrap();
        f.dir.set_org_member(f.acme, f.carol, Some(OrgRole::BillingManager)).unwrap();
        assert_eq!(f.dir.effective_role(Some(f.bob), f.api), Some(Role::Write));
        assert_eq!(f.dir.effective_role(Some(f.carol), f.api), None);
        assert!(!f.dir.is_enterprise_member(f.ent, f.carol));
    }

    #[test]
    fn grants_inherit_down_the_tree_and_take_the_max() {
        let mut f = fixture();
        f.dir.grant(Principal::User(f.bob), Resource::Org(f.acme), Role::Triage).unwrap();
        f.dir.grant(Principal::User(f.bob), Resource::Org(f.platform), Role::Maintain).unwrap();
        f.dir.grant(Principal::Team(f.devs), Resource::Repo(f.api), Role::Write).unwrap();
        assert_eq!(f.dir.effective_role(Some(f.bob), f.api), Some(Role::Maintain));
        assert_eq!(f.dir.effective_role(Some(f.bob), f.site), Some(Role::Triage));
        // re-granting replaces rather than stacks
        f.dir.grant(Principal::User(f.bob), Resource::Org(f.platform), Role::Read).unwrap();
        assert_eq!(f.dir.effective_role(Some(f.bob), f.api), Some(Role::Write));
        f.dir.revoke(Principal::Team(f.devs), Resource::Repo(f.api));
        assert_eq!(f.dir.effective_role(Some(f.bob), f.api), Some(Role::Triage)); // still has acme's
    }

    #[test]
    fn outside_collaborators_get_repo_grants_but_not_org_grants() {
        let mut f = fixture();
        assert!(f.dir.grant(Principal::User(f.carol), Resource::Org(f.acme), Role::Read).is_err());
        f.dir.grant(Principal::User(f.carol), Resource::Repo(f.api), Role::Write).unwrap();
        assert_eq!(f.dir.effective_role(Some(f.carol), f.api), Some(Role::Write));
        assert!(!f.dir.is_org_member(f.acme, f.carol));
    }

    #[test]
    fn nested_teams_inherit_parent_access() {
        let mut f = fixture();
        let backend = f.dir.add_team(f.acme, "backend", Some(f.devs), TeamPrivacy::Visible).unwrap();
        f.dir.set_org_member(f.acme, f.carol, Some(OrgRole::Member)).unwrap();
        f.dir.set_team_member(backend, f.carol, Some(TeamRole::Maintainer)).unwrap();
        f.dir.grant(Principal::Team(f.devs), Resource::Repo(f.api), Role::Write).unwrap();
        assert_eq!(f.dir.effective_role(Some(f.carol), f.api), Some(Role::Write));
        // but not the other way round
        f.dir.grant(Principal::Team(backend), Resource::Repo(f.api), Role::Admin).unwrap();
        assert_eq!(f.dir.effective_role(Some(f.bob), f.api), Some(Role::Write));
    }

    #[test]
    fn team_rules() {
        let mut f = fixture();
        let secret = f.dir.add_team(f.acme, "secret", None, TeamPrivacy::Secret).unwrap();
        assert!(f.dir.add_team(f.acme, "x", Some(secret), TeamPrivacy::Visible).is_err());
        assert!(f.dir.add_team(f.acme, "y", Some(f.devs), TeamPrivacy::Secret).is_err());
        assert!(f.dir.set_team_member(f.devs, f.carol, Some(TeamRole::Member)).is_err());
        let other = f.dir.add_org("other", None, Visibility::Public).unwrap();
        let theirs = f.dir.add_repo(other, "x", Visibility::Public).unwrap();
        assert!(f.dir.grant(Principal::Team(f.devs), Resource::Repo(theirs), Role::Read).is_err());
    }

    #[test]
    fn removing_a_member_prunes_their_access() {
        let mut f = fixture();
        f.dir.grant(Principal::User(f.bob), Resource::Repo(f.api), Role::Write).unwrap();
        f.dir.grant(Principal::Team(f.devs), Resource::Org(f.acme), Role::Read).unwrap();
        f.dir.set_org_member(f.acme, f.bob, None).unwrap();
        assert!(!f.dir.team(f.devs).unwrap().members.contains_key(&f.bob));
        assert_eq!(f.dir.effective_role(Some(f.bob), f.api), None);
        // an inherited membership from above survives removal below
        f.dir.set_org_member(f.acme, f.carol, Some(OrgRole::Member)).unwrap();
        f.dir.set_org_member(f.platform, f.carol, Some(OrgRole::Member)).unwrap();
        f.dir.set_org_member(f.platform, f.carol, None).unwrap();
        assert!(f.dir.is_org_member(f.platform, f.carol));
    }

    #[test]
    fn last_owner_cant_leave() {
        let mut f = fixture();
        assert!(f.dir.set_org_member(f.acme, f.alice, None).is_err());
        assert!(f.dir.set_org_member(f.acme, f.alice, Some(OrgRole::Member)).is_err());
        f.dir.set_org_member(f.acme, f.bob, Some(OrgRole::Owner)).unwrap();
        f.dir.set_org_member(f.acme, f.alice, None).unwrap();
    }

    #[test]
    fn custom_roles_add_permissions_on_a_base() {
        let mut f = fixture();
        let hooks = f.dir.add_custom_role(f.acme, "hook-admin", Role::Read, &["manage_webhooks"]).unwrap();
        assert!(f.dir.add_custom_role(f.acme, "Write", Role::Write, &[]).is_err());
        f.dir.grant(Principal::Team(f.devs), Resource::Repo(f.api), GrantRole::Custom(hooks)).unwrap();
        let access = f.dir.effective_access(Some(f.bob), f.api);
        assert_eq!(access.role, Some(Role::Read));
        assert!(access.permissions.contains("manage_webhooks"));
        // only usable beneath the defining org
        let other = f.dir.add_org("other", None, Visibility::Public).unwrap();
        let theirs = f.dir.add_repo(other, "x", Visibility::Public).unwrap();
        assert!(f.dir.grant(Principal::User(f.carol), Resource::Repo(theirs), GrantRole::Custom(hooks)).is_err());
    }

    #[test]
    fn enterprise_roles_grant_no_repo_access() {
        let mut f = fixture();
        f.dir.set_enterprise_role(f.ent, f.carol, Some(EnterpriseRole::Owner)).unwrap();
        assert_eq!(f.dir.enterprise_role(f.ent, f.carol), Some(EnterpriseRole::Owner));
        assert_eq!(f.dir.effective_role(Some(f.carol), f.api), None);
    }

    #[test]
    fn managed_users_are_confined_to_their_enterprise() {
        let mut f = fixture();
        let emu = f.dir.add_enterprise("globex", Some("gx")).unwrap();
        let gx = f.dir.add_org("globex", None, Visibility::Public).unwrap();
        f.dir.set_org_enterprise(gx, Some(emu)).unwrap();
        let hank = f.dir.add_managed_user(emu, "hank").unwrap();
        assert_eq!(f.dir.account(hank).unwrap().slug, "hank_gx");
        assert!(f.dir.add_managed_user(f.ent, "x").is_err()); // unmanaged enterprise

        f.dir.set_org_member(gx, hank, Some(OrgRole::Owner)).unwrap();
        // outsiders can't get in, and hank can't get out
        assert!(f.dir.set_org_member(gx, f.carol, Some(OrgRole::Member)).is_err());
        let gx_repo = f.dir.add_repo(gx, "x", Visibility::Private).unwrap();
        assert!(f.dir.grant(Principal::User(f.carol), Resource::Repo(gx_repo), Role::Read).is_err());
        assert!(f.dir.set_org_member(f.acme, hank, Some(OrgRole::Member)).is_err());
        let public = f.dir.add_repo(f.carol, "pub", Visibility::Public).unwrap();
        assert!(f.dir.grant(Principal::User(hank), Resource::Repo(public), Role::Write).is_err());
        assert_eq!(f.dir.effective_role(Some(hank), public), Some(Role::Read));
        // no public repos of their own
        assert!(f.dir.add_repo(hank, "p", Visibility::Public).is_err());
        assert!(f.dir.add_repo(hank, "p", Visibility::Private).is_ok());
        // an org with outside members can't move into a managed enterprise
        assert!(f.dir.set_org_enterprise(f.acme, Some(emu)).is_err());
        assert_eq!(f.dir.enterprise_of(f.acme), Some(f.ent));
    }

    #[test]
    fn blocks() {
        let mut f = fixture();
        let gl = f.dir.add_org("gl", None, Visibility::Public).unwrap();
        let open = f.dir.add_repo(gl, "open", Visibility::Public).unwrap();
        f.dir.grant(Principal::User(f.carol), Resource::Repo(open), Role::Write).unwrap();
        f.dir.block(gl, f.carol).unwrap();
        assert_eq!(f.dir.effective_role(Some(f.carol), open), Some(Role::Read));
        assert!(f.dir.grant(Principal::User(f.carol), Resource::Repo(open), Role::Write).is_err());
        assert!(f.dir.block(f.acme, f.bob).is_err()); // still a member
        f.dir.unblock(gl, f.carol);
        f.dir.grant(Principal::User(f.carol), Resource::Repo(open), Role::Write).unwrap();
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
        let mut f = fixture();
        let hooks = f.dir.add_custom_role(f.acme, "hook-admin", Role::Read, &["manage_webhooks"]).unwrap();
        f.dir.grant(Principal::Team(f.devs), Resource::Repo(f.api), GrantRole::Custom(hooks)).unwrap();
        f.dir.grant(Principal::User(f.bob), Resource::Repo(f.site), Role::Write).unwrap();
        let json = serde_json::to_string(&f.dir).unwrap();
        let back: Directory = serde_json::from_str(&json).unwrap();
        assert_eq!(back.repo_path(f.api), f.dir.repo_path(f.api));
        assert_eq!(back.effective_access(Some(f.bob), f.api), f.dir.effective_access(Some(f.bob), f.api));
        assert_eq!(back.effective_role(Some(f.bob), f.site), Some(Role::Write));
    }
}
