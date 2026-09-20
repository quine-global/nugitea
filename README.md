# nugitea

A very very minimal Gitea. No users, no roles, no login, no web UI — just a
server that can clone, push, and mirror git repos, implemented by shelling
out to `git` the same way Gitea itself does (verified against Gitea's actual
source: `routers/web/repo/githttp.go`, `modules/ssh/ssh.go`, `cmd/serv.go`,
`modules/gitrepo/hooks.go`, `services/mirror/`).

## What it does

- **Smart-HTTP** clone/push (`git clone http://host:3080/name.git`) via
  `git upload-pack`/`git receive-pack --stateless-rpc`, same as Gitea's
  `POST /info/refs`, `/git-upload-pack`, `/git-receive-pack` endpoints.
- **SSH** clone/push (`git clone ssh://host:2222/name.git`) via a built-in
  SSH server that execs the same git subcommands directly against the
  session, mirroring Gitea's `modules/ssh` + `cmd/serv.go`.
- **Git hooks**: bare repos get `pre-receive`/`update`/`post-receive`
  delegator scripts that call back into `nugitea hook <name>`, mirroring
  Gitea's hook delegation — currently a no-op stub (no protected branches,
  no webhooks), a landing spot for future policy.
- **Pull mirrors**: periodically `git fetch --prune --tags` from a configured
  remote into a local bare repo.
- **Push mirrors**: periodically `git push --mirror -f` a local bare repo to
  a configured remote.

## What it doesn't do

Everything else: no users, no authentication, no web UI, no issues/PRs, no
webhooks, no LFS, no protected branches, no dumb-HTTP fallback, no AGit. All
repos are fully open to clone and push. See `internal/auth` for the seam
where real access control could be added later without touching the HTTP or
SSH transport code.

## Usage

```sh
go build -o nugitea ./cmd/nugitea

# Start the server (repos live under ./data by default)
./nugitea serve --http :3080 --ssh :2222 --repo-root ./data

# In another terminal:
./nugitea repo create demo
git clone http://localhost:3080/demo.git
git clone ssh://localhost:2222/demo.git   # port 2222 must be reachable as your ssh client's target

# Mirroring (flags must come before the positional args)
./nugitea mirror add-pull --interval 5m demo https://example.com/some/repo.git
./nugitea mirror add-push --interval 5m demo git@example.com:backup/demo.git
```

`NUGITEA_REPO_ROOT` sets the repo root for the `repo`/`mirror` subcommands
when not passed `--repo-root` (only `serve` takes that flag directly; the
other subcommands assume they're run against the same root as a running
server).

## Layout

```
cmd/nugitea/        CLI entry point (serve, repo, mirror, hook subcommands)
internal/gitcmd/    git subprocess builder (Gitea's modules/git/gitcmd, trimmed)
internal/repo/      bare-repo layout, git init --bare, hook installation
internal/httpgit/   smart-HTTP protocol handler
internal/sshgit/    SSH server
internal/mirror/    pull/push mirror config + periodic scheduler
internal/auth/      access-control seam (AllowAll today)
```
