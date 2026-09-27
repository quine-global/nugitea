#!/usr/bin/env bash
# Seeds demo content against a running nugitea instance, so a fresh
# `docker compose up` / local dev instance has something to look at
# instead of an empty /repos page: an org `demo` with a nested org
# `demo/tools`, and one repo in each. Uses the same client-visible path a
# real user would: `nugitea org/repo create` + an actual `git
# commit`/push, not a shortcut that pokes the storage tier's disk
# directly. Safe to re-run — skips anything that already exists.
#
# Usage: scripts/seed-fixtures.sh [app-http-url]
#
# The admin commands have to run where the app tier's state dir
# (accounts.json) lives, so NUGITEA is the command to run them with:
#   - Docker Compose (default): NUGITEA="docker compose exec -T app nugitea"
#   - local cargo build:        NUGITEA=./target/release/nugitea, with
#     NUGITEA_STATE_DIR / NUGITEA_STORAGE set to match `nugitea serve`
set -euo pipefail

APP_URL="${1:-http://localhost:3080}"
# Intentionally word-split: it's a command plus arguments.
read -r -a NUGITEA <<< "${NUGITEA:-docker compose exec -T app nugitea}"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

seed_org() {
  local path="$1"
  if "${NUGITEA[@]}" org list | grep -qx "$path"; then
    echo "skip org $path (already exists)"
    return
  fi
  "${NUGITEA[@]}" org create "$path"
}

seed_repo() {
  local path="$1" file="$2" content="$3"

  if "${NUGITEA[@]}" repo list | grep -qx "$path"; then
    echo "skip $path (already exists)"
    return
  fi

  "${NUGITEA[@]}" repo create "$path"

  local work="$tmp/${path//\//_}"
  git init -q -b main "$work"
  printf '%s\n' "$content" > "$work/$file"
  git -C "$work" -c user.name=nugitea -c user.email=nugitea@localhost add "$file"
  git -C "$work" -c user.name=nugitea -c user.email=nugitea@localhost commit -q -m "Initial commit"
  git -C "$work" push -q "$APP_URL/$path.git" main
  echo "seeded $path"
}

seed_org demo
seed_org demo/tools
seed_repo demo/foobar hello.txt "hello"
seed_repo demo/tools/barbaz sup.txt "sup"
