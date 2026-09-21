#!/usr/bin/env bash
# Seeds two demo repos (foobar, barbaz) against a running nugitea instance,
# so a fresh `docker compose up` / local dev instance has something to look
# at instead of an empty /repos page. Uses the same client-visible path a
# real user would: `nugitea repo create` + an actual `git commit`/push, not
# a shortcut that pokes the storage tier's disk directly.
#
# Usage: scripts/seed-fixtures.sh [app-http-url] [storage-http-url]
# Defaults match `docker compose up`'s published ports.
set -euo pipefail

APP_URL="${1:-http://localhost:3080}"
STORAGE_URL="${2:-http://localhost:9080}"
NUGITEA_BIN="${NUGITEA_BIN:-nugitea}"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

seed_repo() {
  local name="$1" file="$2" content="$3"

  if "$NUGITEA_BIN" repo list --storage "$STORAGE_URL" | grep -qx "$name"; then
    echo "skip $name (already exists)"
    return
  fi

  "$NUGITEA_BIN" repo create "$name" --storage "$STORAGE_URL"

  local work="$tmp/$name"
  git init -q -b main "$work"
  printf '%s\n' "$content" > "$work/$file"
  git -C "$work" -c user.name=nugitea -c user.email=nugitea@localhost add "$file"
  git -C "$work" -c user.name=nugitea -c user.email=nugitea@localhost commit -q -m "Initial commit"
  git -C "$work" push -q "$APP_URL/$name.git" main
  echo "seeded $name"
}

seed_repo foobar hello.txt "hello"
seed_repo barbaz sup.txt "sup"
