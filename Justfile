# List available project commands.
default:
    @just --list

# Run the frontend and backend development processes.
dev:
    process-compose -f process-compose.yaml up

# Run only the frontend rebuild watcher.
frontend:
    process-compose -f process-compose.yaml up frontend

# Run only the Rust rebuild watcher.
backend:
    process-compose -f process-compose.yaml up backend --no-deps

# Install locked frontend dependencies.
install:
    bun install --cwd frontend --frozen-lockfile

# Build the static frontend.
frontend-build:
    bun run --cwd frontend build

# Build the frontend, then the Rust application.
build: frontend-build
    cargo build

# Check frontend types and Rust compilation.
check:
    bun run --cwd frontend check
    cargo check

# Run the backend test suite.
test:
    cargo test

# Build release artifacts.
release-build:
    ./scripts/build-release.sh

# Refresh the pinned frontend dependency hash.
frontend-hash:
    ./scripts/update-frontend-hash.sh
