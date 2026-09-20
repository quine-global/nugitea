//! The extension seam for access control. nugitea ships with no users or
//! login, so the only implementation today is AllowAll, but the httpgit and
//! sshgit transports depend on this trait rather than a concrete decision
//! so real auth can be added later without touching them.

/// Decides whether an operation on a repo is permitted.
pub trait Authorizer: Send + Sync {
    fn allow_pull(&self, repo: &str) -> bool;
    fn allow_push(&self, repo: &str) -> bool;
}

/// Permits every pull and push. The default Authorizer.
pub struct AllowAll;

impl Authorizer for AllowAll {
    fn allow_pull(&self, _repo: &str) -> bool {
        true
    }
    fn allow_push(&self, _repo: &str) -> bool {
        true
    }
}
