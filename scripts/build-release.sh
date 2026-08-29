#!/usr/bin/env sh
set -eu

bun install --cwd frontend --frozen-lockfile

# Vite does not empty frontend/build, so a leftover chunk can keep the
# hydration global of an earlier build while index.html gets a fresh one, and
# the release binary then embeds a page that loads to a blank body.
rm -rf frontend/build
bun run --cwd frontend build
cargo build --release --locked
