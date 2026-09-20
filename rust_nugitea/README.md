# rust_nugitea

A tokio/Rust rewrite of the sibling Go `nugitea` implementation, one
directory up. Same scope, same behavior, same design: no users, no auth, no
web UI — smart-HTTP and SSH clone/push against bare repos, plus periodic
pull/push mirroring, all implemented by shelling out to `git`.

- **HTTP** (`src/httpgit.rs`): `axum` router streaming request/response
  bodies directly into/out of `git upload-pack`/`receive-pack
  --stateless-rpc` subprocesses.
- **SSH** (`src/sshgit.rs`): `russh` server execing the same git subcommands
  directly against the session, with client stdin forwarded via an
  `mpsc` channel and subprocess stdout/stderr streamed back via
  `russh::server::Handle`.
- **Hooks** (`src/repo.rs`): same delegator-script approach as the Go
  version — `hooks/{pre-receive,update,post-receive}` call back into
  `nugitea hook <name>`, currently a no-op stub.
- **Mirrors** (`src/mirror/`): a `tokio::time::interval` scheduler runs
  `git fetch --prune --tags` (pull) / `git push --mirror -f` (push) against
  remotes configured via `git remote add --mirror=fetch|push`.
- **Auth** (`src/auth.rs`): an `Authorizer` trait defaulting to `AllowAll`,
  the same extension seam as the Go version.

## Usage

Identical CLI surface to the Go binary:

```sh
cargo build --release
./target/release/nugitea serve --http :3080 --ssh :2222 --repo-root ./data

./target/release/nugitea repo create demo
git clone http://localhost:3080/demo.git
git clone ssh://localhost:2222/demo.git

./target/release/nugitea mirror add-pull --interval 5m demo https://example.com/some/repo.git
./target/release/nugitea mirror add-push --interval 5m demo git@example.com:backup/demo.git
```

`NUGITEA_REPO_ROOT` sets the repo root for the `repo`/`mirror` subcommands,
same as the Go version. Durations accept a single `s`/`m`/`h` suffix (e.g.
`30s`, `5m`, `2h`) — a deliberately smaller subset of Go's duration syntax.
