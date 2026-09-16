# Contributing to Gitadel

Focused bug fixes and features that fit Gitadel's small-forge scope are welcome. Gitadel intentionally does not aim to grow pull requests, issues, social features, or browser-based editing.

## Development environment

The Nix flake pins Bun, Rust, and the native dependencies. Enter the shell and
install the locked frontend dependencies:

```bash
direnv allow
# or: nix develop
just install
```

Start the complete development loop in one terminal:

```bash
just dev
```

Process Compose runs the frontend and backend watchers, waits for the first
frontend build before starting Gitadel, combines their logs, and stops both
process groups on exit. Use `just frontend` or `just backend` to run one watcher.
Run `just --list` for the remaining build and maintenance commands.

### Single-port development

Only the backend binds a port and serves the frontend. OAuth callbacks, cookies,
and passkeys therefore use one origin. In debug builds the server reads
`frontend/build/` from disk on every request, so frontend rebuilds are live and
only Rust changes restart the server. The frontend watcher runs a fresh,
finite production build after each relevant source change. See
[AGENTS.md](AGENTS.md) for the underlying commands.

`bun run --cwd frontend dev` serves a second origin that proxies only `/api`
and `/healthz`, so backend routes are unreachable through it.

Bare defaults are `127.0.0.1:3000` with SSH on `2222`, a `gitadel.db` in the
repository root, and `repositories/`, `lfs/`, and an SSH host key beside it -
all gitignored. Those ports collide with any Gitadel already running on the
machine, so give a dev instance its own ports and paths through a gitignored
`gitadel.toml` in the root, which is read automatically, or through the CLI
flags and their `GITADEL_*` environment variables. See [AGENTS.md](AGENTS.md).

## Validation

Run formatting, linting, type checking, and the frontend production build before submitting a change:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
bun run --cwd frontend check
bun run --cwd frontend build
```

Nix only includes Git-tracked files in flake source inputs. Stage new source files before investigating a Nix build that cannot find them.

## Production build

Build the embedded SvelteKit frontend and both release binaries with:

```bash
just release-build
```

The outputs are `target/release/gitadel` (server and offline maintenance) and `target/release/gitadel-cli` (token-authenticated remote client). To build only the client, run `cargo build --release --locked -p gitadel-cli`; it does not need a frontend build.

Git, Git LFS, and OpenSSH remain in the development shell for interoperability checks. They are not server runtime dependencies. Native Git operations use the immutable Sley fork revision in `Cargo.toml`; dependency updates must also refresh `Cargo.lock` and the Nix Git-source hash.

The Nix package installs frontend dependencies in a fixed-output derivation. Refresh its hash whenever `frontend/bun.lock` changes:

```bash
just frontend-hash
```

## Repository layout

```text
src/            Rust server, offline maintenance, identity, Git, SSH, HTTP, and LFS
cli/            Token-authenticated remote command-line client
frontend/       SvelteKit web interface
nix/            Separate NixOS server and client modules
scripts/        Release and Nix dependency-hash helpers
docs/research/  Product and integration research
```

Release-facing changes belong in [CHANGELOG.md](CHANGELOG.md).
