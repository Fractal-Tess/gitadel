#!/usr/bin/env sh
set -eu

bun install --cwd frontend --frozen-lockfile
bun run --cwd frontend build
cargo build --release --locked
