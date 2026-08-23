#!/usr/bin/env bash
# Local development: serve the API and a live-rebuilt frontend from one port.
#
# Starts SvelteKit in build-watch mode so every frontend change lands in
# frontend/build/. The Gitadel binary reads those assets from disk on each
# request in debug builds (rust-embed embeds them at compile time only in
# release builds), so frontend changes are picked up without restarting it.
# Rust changes still require restarting this script.
set -euo pipefail

cd "$(dirname "$0")/.."

for tool in bun cargo; do
  command -v "$tool" >/dev/null || {
    echo "$tool is required" >&2
    exit 1
  }
done

bind="${GITADEL_BIND:-127.0.0.1:8080}"

# Always rebuild from an empty directory: an interrupted watch build can leave
# frontend/build holding an index.html and JS chunks from different builds,
# which loads but never boots (the SvelteKit hydration global stops matching).
echo "==> Building frontend from clean"
rm -rf frontend/build
bun run --cwd frontend build

echo "==> Watching frontend changes"
bun run --cwd frontend build --watch &
watcher_pid=$!
trap 'kill "$watcher_pid" 2>/dev/null || true' EXIT

# WebAuthn rejects bare-IP public URLs, so advertise localhost even though
# the listener binds the loopback address.
echo "==> Starting Gitadel on ${bind}"
cargo run -- \
  --bind "${bind}" \
  --public-url "${GITADEL_PUBLIC_URL:-http://localhost:${bind##*:}}" \
  --database-url "${GITADEL_DATABASE_URL:-sqlite://data/dev.db?mode=rwc}" \
  --repository-root "${GITADEL_REPOSITORY_ROOT:-data/repositories}" \
  --lfs-root "${GITADEL_LFS_ROOT:-data/lfs}" \
  --ssh-bind "${GITADEL_SSH_BIND:-127.0.0.1:2322}" \
  --ssh-host-key "${GITADEL_SSH_HOST_KEY:-data/ssh-host-dev}" \
  "$@"
