# Contributing to Gitadel

Focused bug fixes and features that fit Gitadel's small-forge scope are welcome. Gitadel intentionally does not aim to grow pull requests, issues, social features, or browser-based editing.

## Development environment

The repository pins Bun, Rust, and native dependencies through [devenv](https://devenv.sh/):

```bash
devenv shell
frontend-install
```

Run the frontend and backend in separate shells:

```bash
frontend       # SvelteKit on http://localhost:5173
backend        # Gitadel HTTP on :3000 and SSH on :2222
```

`nix develop` provides an equivalent shell without devenv, although it does not pin the Rust toolchain as precisely.

## Checks

Run formatting, linting, tests, and the frontend production build before submitting a change:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
bun run --cwd frontend build
```

The complete Nix check builds the package and boots a VM that verifies the NixOS module can start Gitadel, bootstrap an administrator, and survive a restart:

```bash
nix flake check
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
