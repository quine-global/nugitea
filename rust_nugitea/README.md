# rust_nugitea

A tokio/Rust rewrite of the sibling Go `nugitea` implementation, one
directory up — but split into two tiers the way GitLab splits the Rails app
from Gitaly, instead of the single-process design Gitea (and the Go
version) use. That split is the point: it's what lets the app tier scale
out horizontally without needing shared/replicated filesystem storage.

```
git client --HTTP/SSH--> [nugitea: app tier]  --internal API-->  [nugitea-storaged: storage tier]
                           httpgit, sshgit,                         owns repo-root disk,
                           mirror scheduler,                        runs git subprocesses,
                           Authorizer (auth seam)                   installs hooks
```

The app tier never touches a local git path or subprocess — it only knows
repo *names* and proxies git protocol bytes to the storage tier. The
storage tier trusts its caller (no auth of its own), the same way Gitaly
trusts the Rails app tier; `Authorizer` checks happen once, at the app
tier, before a request is ever proxied. Still no users, no login — just
clone, push, mirror, and a minimal read-only file-browser web UI.

Two binaries, one crate (`src/lib.rs` + `src/bin/*.rs`), always run
together — there's no embedded single-process mode:

- **`nugitea` (app tier)**: `httpgit.rs` (smart-HTTP) and `sshgit.rs` (SSH)
  are thin reverse proxies — validate the repo name, check `Authorizer`,
  forward to the storage tier. The mirror scheduler (`mirror/`) lives here
  too; `mirrors.json` is app-tier state (schedule bookkeeping, not git
  data), while the actual `git fetch`/`git push --mirror` calls are proxied
  through `storage_client.rs`.
- **`nugitea-storaged` (storage tier)**: owns the repo-root disk
  (`storage.rs`), installs hooks, and exposes two internal listeners:
  - an HTTP API (`storage_server.rs`) for everything that's naturally
    request-then-response — ref advertisement, the HTTP transport's
    `--stateless-rpc` upload-pack/receive-pack, repo create/list/exists,
    and the generic `run_git` used by mirror sync.
  - a raw TCP relay (`git_exec_tcp.rs`) just for the SSH transport's
    interactive git protocol. SSH-driven `git upload-pack`/`receive-pack`
    write a ref advertisement to stdout *before* reading anything from
    stdin, with both directions live simultaneously — a duplex pattern
    HTTP/1.1 request/response framing doesn't reliably support. A plain
    TCP socket does, the same shape as talking to a local subprocess's
    pipes, just across a network hop.

`names.rs` holds the one shared, pure (no I/O) repo-name validation and
wire-form parsing used by both tiers. `webui.rs` is the file browser —
Leptos in SSR-only mode (`.to_html()` on a composed view, no WASM/
hydration/`cargo-leptos`, since every click here is a plain link → full
page load, not client-side state) rendering `GET /{repo}`,
`/{repo}/tree/{ref}/...`, and `/{repo}/blob/{ref}/...`, backed by new
`ls-tree`/`show`/`for-each-ref` plumbing in `tree.rs` on the storage tier.

## Running it: two processes, always

There's no single-process mode — `nugitea-storaged` (owns the disk) and
`nugitea` (speaks git + serves the file browser) are always separate, even
on one machine. Two ways to run them:

### Docker Compose (recommended for trying it out)

```sh
docker compose up --build
```

This builds both binaries into one image (`Dockerfile`) and starts both
services (`docker-compose.yml`) wired together on the compose-internal
network. The app tier's `3080` (HTTP) and `2222` (SSH) are published, and
so are `storaged`'s `9080` (HTTP API) and `9081` (git-exec TCP relay) —
that API has no auth of its own (it trusts its caller, the app tier's
`Authorizer` checks happen on the other side of the proxy), so publishing
it means anyone who can reach those ports can read or write any repo
directly, without going through the app tier at all. Fine for local or
otherwise trusted-network use; drop the `storaged` service's `ports:` block
in `docker-compose.yml` before exposing this any more widely, so it's only
reachable from the `app` service over the compose-internal network. Repo
data and mirror state persist in named volumes (`nugitea-data`,
`nugitea-state`) across restarts.

Once it's up:

```sh
docker compose exec app nugitea repo create demo --storage http://storaged:9080
git clone http://localhost:3080/demo.git
git clone ssh://localhost:2222/demo.git
# push something, then browse it:
open http://localhost:3080/demo        # or just visit it in a browser
```

### Building and running directly with cargo

```sh
cargo build --release

# storage tier — owns the disk
./target/release/nugitea-storaged serve --http 127.0.0.1:9080 --tcp 127.0.0.1:9081 --repo-root ./data

# app tier — speaks git to the outside world and serves the file browser
./target/release/nugitea serve --http :3080 --ssh :2222 \
    --storage http://127.0.0.1:9080 --storage-tcp 127.0.0.1:9081 --state-dir ./state

./target/release/nugitea repo create demo --storage http://127.0.0.1:9080
git clone http://localhost:3080/demo.git
git clone ssh://localhost:2222/demo.git

./target/release/nugitea mirror add-pull --interval 5m --storage http://127.0.0.1:9080 \
    --state-dir ./state demo https://example.com/some/repo.git
./target/release/nugitea mirror add-push --interval 5m --storage http://127.0.0.1:9080 \
    --state-dir ./state demo git@example.com:backup/demo.git
```

Once something's been pushed, browse it at `http://localhost:3080/demo` —
that redirects to the default branch (`main` if it exists, else `master`,
else whatever branch sorts first) and lets you walk the tree and view file
contents.

Env var equivalents: `NUGITEA_STORAGE` / `NUGITEA_STORAGE_TCP` (app tier's
`--storage`/`--storage-tcp`), `NUGITEA_STATE_DIR` (app tier's
`--state-dir`), `NUGITEA_REPO_ROOT` (storage tier's `repo`/`hook`
subcommands). `nugitea-storaged repo create/list` also works directly on
the storage node, bypassing the HTTP API, for operators logged in there.
Durations accept a single `s`/`m`/`h` suffix (e.g. `30s`, `5m`, `2h`) — a
deliberately smaller subset of Go's duration syntax.

## Known simplifications from the single-process version

- The SSH transport no longer forwards the git subprocess's raw OS-level
  stderr to the client as extended data — `remote: ...` messages during
  push/hooks travel through git's own sideband multiplexing *inside*
  stdout, which is unaffected; only git's own rare fatal-error stderr
  output would be missed. This also makes SSH consistent with HTTP, which
  never forwarded that channel either.
- The exact git subprocess exit code doesn't cross the proxy hop. SSH now
  reports exit status 0 on a successful proxy round-trip, 1 on failure —
  approximating "did the request go through," matching how HTTP has never
  surfaced git's literal exit code to its client, only logged it
  server-side.
