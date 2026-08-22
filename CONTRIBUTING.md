# Contributing to Gitadel

Focused bug fixes and features that fit Gitadel's small-forge scope are welcome. Gitadel intentionally does not aim to grow pull requests, issues, social features, or browser-based editing.

## Development environment

The Nix flake pins Bun, the Rust toolchain from `rust-toolchain.toml`, and all native dependencies. Enter it automatically with direnv or manually with Nix:

```bash
direnv allow
# or: nix develop
frontend-install
```

Run the frontend and backend in separate shells:

```bash
frontend       # SvelteKit on http://localhost:5173
backend        # Gitadel HTTP on :3000 and SSH on :2222
```

The flake also provides `frontend-build`, `release-build`, and `frontend-hash` commands.

## Validation

Run formatting, linting, type checking, and the frontend production build before submitting a change:

```bash
cargo fmt --all -- --check
cargo clippy --bin gitadel --locked -- -D warnings
bun run --cwd frontend check
bun run --cwd frontend build
```

Nix only includes Git-tracked files in flake source inputs. Stage new source files before investigating a Nix build that cannot find them.

## Production build

Build the embedded SvelteKit frontend and release binary with:

```bash
release-build
```

The Nix package installs frontend dependencies in a fixed-output derivation. Refresh its hash whenever `frontend/bun.lock` changes:

```bash
frontend-hash
# or
./scripts/update-frontend-hash.sh
```

## Repository layout

```text
src/            Rust server, CLI, identity, repository, SSH, HTTP, and LFS code
frontend/       SvelteKit web interface
nix/            NixOS service module
scripts/        Release and Nix dependency-hash helpers
docs/research/  Product and integration research
```

Release-facing changes belong in [CHANGELOG.md](CHANGELOG.md).
