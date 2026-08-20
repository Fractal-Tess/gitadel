#!/usr/bin/env sh
set -eu

bun run --cwd frontend build
cargo build --release
