//! Maps a client-requested git service to the git subcommand argument and
//! the Authorizer check it requires. Shared by the HTTP and SSH transports
//! so the upload-pack/receive-pack table exists exactly once, instead of
//! each transport encoding its own copy in a different string vocabulary.

use crate::auth::Authorizer;

#[derive(Clone, Copy)]
pub enum Service {
    UploadPack,
    ReceivePack,
}

impl Service {
    /// The argument passed to `git` (e.g. `git upload-pack`), also used as
    /// the HTTP service name suffix and Content-Type fragment.
    pub fn git_arg(self) -> &'static str {
        match self {
            Service::UploadPack => "upload-pack",
            Service::ReceivePack => "receive-pack",
        }
    }

    pub fn allow(self, auth: &dyn Authorizer, repo: &str) -> bool {
        match self {
            Service::UploadPack => auth.allow_pull(repo),
            Service::ReceivePack => auth.allow_push(repo),
        }
    }

    /// Parses the HTTP smart-protocol service name, e.g. "upload-pack" (the
    /// `service=git-upload-pack` query param with its `git-` prefix already
    /// stripped).
    pub fn from_http_param(s: &str) -> Option<Service> {
        match s {
            "upload-pack" => Some(Service::UploadPack),
            "receive-pack" => Some(Service::ReceivePack),
            _ => None,
        }
    }

    /// Parses the SSH exec verb, e.g. "git-upload-pack".
    pub fn from_ssh_verb(s: &str) -> Option<Service> {
        match s {
            "git-upload-pack" => Some(Service::UploadPack),
            "git-receive-pack" => Some(Service::ReceivePack),
            _ => None,
        }
    }
}
