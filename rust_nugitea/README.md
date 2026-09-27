# rust_nugitea

A tokio/Rust rewrite of the sibling Go `nugitea` implementation, one
directory up — but split into two tiers the way GitLab splits the Rails app
from Gitaly, instead of the single-process design Gitea (and the Go
version) use. That split is the point: it's what lets the app tier scale
out horizontally without needing shared/replicated filesystem storage.

```
git client --HTTP/SSH--> [nugitea: app tier]  --internal API-->  [nugitea-storaged: storage tier]
                           httpgit, sshgit,                         owns repo-root disk,
                           GraphQL API,                             runs git subprocesses,
                           mirror scheduler,                        installs hooks
                           Authorizer (auth seam)

                          [web: Nuxt frontend]  --/graphql-->  [nugitea: app tier]
                           isomorphic SSR + client-side
                           routing/hydration, no git
                           protocol knowledge at all
```

The app tier never touches a local git path or subprocess — it only knows
repo *paths* (`owner/name`, or `org/sub-org/.../name` under nested orgs)
and proxies git protocol bytes to the storage tier. The storage tier
trusts its caller (no auth of its own), the same way Gitaly trusts the
Rails app tier; `Authorizer` checks happen once, at the app tier, before
a request is ever proxied. Repos have owners — users and (optionally
nested) orgs — but there's still no login, so nothing enforces who can
do what yet: just clone, push, mirror, and a minimal read-only file
browser.

The browsing UI is a third, independent piece: a Nuxt.js app (`web/`) that
speaks only GraphQL to the app tier's `/graphql` endpoint. It doesn't know
git exists — it renders whatever the schema gives it, isomorphically
(server-rendered on first load, then hydrated for client-side routing).

Three deployables total, always run separately — there's no embedded
single-process mode:

- **`nugitea` (app tier)**: `httpgit.rs` (smart-HTTP) and `sshgit.rs` (SSH)
  are thin reverse proxies — resolve the repo path against the accounts
  directory (case-insensitively, like GitHub), check `Authorizer`, forward
  to the storage tier. `graphql.rs` exposes a GitHub-compatible GraphQL API
  (`repository(owner:, name:)`, `repositoryOwner`/`user`/`organization(login:)`,
  `resource(url:)`, `object(expression: "ref:path") { ... on Tree | ... on
  Blob }`) — see that file's doc comment for the full schema and the one
  extension (nested orgs). App-tier state lives in `--state-dir`:
  `accounts.json` (users, orgs, repo ownership — see `accounts/`) and
  `mirrors.json` (mirror schedule bookkeeping). The mirror scheduler
  (`mirror/`) lives here too, while the actual `git fetch`/`git push
  --mirror` calls are proxied through `storage_client.rs`.
- **`nugitea-storaged` (storage tier)**: owns the repo-root disk
  (`storage.rs`), installs hooks, and exposes two internal listeners:
  - an HTTP API (`storage_server.rs`) for everything that's naturally
    request-then-response — ref advertisement, the HTTP transport's
    `--stateless-rpc` upload-pack/receive-pack, repo create/list/exists,
    tree/blob/branch/file-list reads for the GraphQL layer, and the generic
    `run_git` used by mirror sync.
  - a raw TCP relay (`git_exec_tcp.rs`) just for the SSH transport's
    interactive git protocol. SSH-driven `git upload-pack`/`receive-pack`
    write a ref advertisement to stdout *before* reading anything from
    stdin, with both directions live simultaneously — a duplex pattern
    HTTP/1.1 request/response framing doesn't reliably support. A plain
    TCP socket does, the same shape as talking to a local subprocess's
    pipes, just across a network hop.
- **`web` (Nuxt frontend)**: pages under `web/app/pages/` — a repo-root
  redirect to the default branch, a tree browser, a blob viewer — plus a
  `SearchBox.vue` component (press `/` to open, fetches
  `repository.files(ref)` once, filters client-side, no server round-trip
  per keystroke). No GraphQL client library (Apollo/urql); just `$fetch`
  posting raw query strings via `web/app/composables/graphql.ts`, matching
  this project's general preference for the minimal tool over a heavier
  abstraction. SSR and post-hydration browser fetches both hit the app
  tier's `/graphql` — SSR resolves it container-to-container
  (`NUXT_GRAPHQL_URL`), the browser resolves it via whatever address the
  host can reach (`NUXT_PUBLIC_GRAPHQL_URL`).

`names.rs` holds the one shared, pure (no I/O) repo-path validation and
wire-form parsing used by both Rust tiers. On disk, a repo lives at
`<repo-root>/<owner path>/<name>.git`.

`accounts/` models users, orgs (optionally nested, GitLab-style), org
membership and base permissions, nested teams, custom repo roles,
enterprises (including managed/EMU ones), blocks, and role grants — one
model covering how Gitea, GitHub, and GitLab arrange them (see its doc
comment for the comparison). Account and repo ownership are live — every
repo path resolves through it — but memberships and grants aren't
enforced yet, since nothing logs in.

## Running it: three processes, always

`nugitea-storaged` (owns the disk), `nugitea` (speaks git + serves
GraphQL), and `web` (Nuxt frontend) are always separate processes, even on
one machine. Two ways to run them:

### Docker Compose (recommended for trying it out)

```sh
docker compose up --build
```

This builds the two Rust binaries into one image (`Dockerfile`) and the
Nuxt app into a separate image (`web/Dockerfile`), then starts all three
services (`docker-compose.yml`) wired together on the compose-internal
network. Published ports: `web`'s `3000` (the browsing UI), the app tier's
`3080` (HTTP git + GraphQL) and `2222` (SSH), and `storaged`'s `9080`
(HTTP API) and `9081` (git-exec TCP relay). `storaged`'s API has no auth of
its own (it trusts its caller — the app tier's `Authorizer` checks happen
on the other side of the proxy), so publishing it means anyone who can
reach those ports can read or write any repo directly, without going
through the app tier at all. Fine for local or otherwise trusted-network
use; drop the `storaged` service's `ports:` block in `docker-compose.yml`
before exposing this any more widely, so it's only reachable from the
`app` service over the compose-internal network. Repo data and mirror
state persist in named volumes (`nugitea-data`, `nugitea-state`) across
restarts.

Once it's up:

```sh
docker compose exec app nugitea user create alice
docker compose exec app nugitea repo create alice/demo
docker compose exec app nugitea org create acme
docker compose exec app nugitea org create acme/platform   # a nested org
docker compose exec app nugitea repo create acme/platform/api --visibility internal
git clone http://localhost:3080/alice/demo.git
git clone ssh://localhost:2222/acme/platform/api.git
# push something, then browse it:
open http://localhost:3000/alice/demo        # or just visit it in a browser
```

Admin commands run inside the `app` container because that's where
`accounts.json` lives (compose sets `NUGITEA_STATE_DIR` and
`NUGITEA_STORAGE` there, so no flags are needed). `org create` and `repo
create` take `--visibility public|internal|private` (default `public`); a
child can't be more visible than its parent.

There are no fixtures baked into the image — a fresh instance starts with
zero repos and an empty `/repos` page. `scripts/seed-fixtures.sh` creates
an org `demo` with a nested org `demo/tools` and a small repo in each, the
same way a real client would — `nugitea org/repo create` followed by an
actual `git commit`/push, not a shortcut that writes to the storage
tier's disk directly — and is safe to re-run (it skips anything that
already exists):

```sh
scripts/seed-fixtures.sh http://localhost:3080
open http://localhost:3000/repos
```

### Building and running directly with cargo + npm

```sh
cargo build --release

# storage tier — owns the disk
./target/release/nugitea-storaged serve --http 127.0.0.1:9080 --tcp 127.0.0.1:9081 --repo-root ./data

# app tier — speaks git to the outside world and serves GraphQL
./target/release/nugitea serve --http :3080 --ssh :2222 \
    --storage http://127.0.0.1:9080 --storage-tcp 127.0.0.1:9081 --state-dir ./state

# admin commands read/write the same state dir as `serve`
export NUGITEA_STATE_DIR=./state NUGITEA_STORAGE=http://127.0.0.1:9080
./target/release/nugitea user create alice
./target/release/nugitea repo create alice/demo
git clone http://localhost:3080/alice/demo.git
git clone ssh://localhost:2222/alice/demo.git

./target/release/nugitea mirror add-pull --interval 5m alice/demo https://example.com/some/repo.git
./target/release/nugitea mirror add-push --interval 5m alice/demo git@example.com:backup/demo.git

# seed demo content (see above)
NUGITEA=./target/release/nugitea scripts/seed-fixtures.sh

# frontend — a separate process, run from web/
cd web && npm install
NUXT_GRAPHQL_URL=http://127.0.0.1:3080/graphql \
NUXT_PUBLIC_GRAPHQL_URL=http://localhost:3080/graphql \
npm run dev
```

Once something's been pushed, browse it at `http://localhost:3000/alice/demo`
(or the Nuxt dev server's own port, `3001` by default) — that redirects to
the default branch (`main` if it exists, else `master`, else whatever
branch sorts first) and lets you walk the tree, view file contents, and
press `/` to search the whole repo's file list in real time. URLs follow
GitHub's shape (`/owner/repo/tree/<ref>/<path>`, `/owner/repo/blob/...`),
with the owner part growing a segment per nested org; `/<owner>` lists an
owner's repos and sub-orgs. The GraphQL
API itself has an interactive GraphiQL page at `GET http://localhost:3080/graphql`.

Env var equivalents for the Rust CLI: `NUGITEA_STORAGE` /
`NUGITEA_STORAGE_TCP` (app tier's `--storage`/`--storage-tcp`),
`NUGITEA_STATE_DIR` (app tier's `--state-dir`), `NUGITEA_REPO_ROOT`
(storage tier's `repo`/`hook` subcommands). `nugitea-storaged repo
create/list` also works directly on the storage node, bypassing the HTTP
API, for operators logged in there — but a repo created that way has no
entry in `accounts.json`, so the app tier won't serve it until one is
added.

Upgrading from before repos had owners: flat `<repo-root>/<name>.git`
repos aren't reachable any more (a repo path needs an owner). Move each
one to `<repo-root>/<owner>/<name>.git` and create the owner and repo
entries with `nugitea user/org create`; `repo create` fails if the
directory already exists, so add the `accounts.json` entry before moving
the repo in place. Durations accept a single `s`/`m`/`h`
suffix (e.g. `30s`, `5m`, `2h`) — a deliberately smaller subset of Go's
duration syntax.

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
