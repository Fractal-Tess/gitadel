# Working on Gitadel

## Rust crate directory names

Use short role names for crate directories while keeping service-prefixed Cargo
package names. This is a project convention, not a Rust or Cargo standard.

The root manifest is a virtual workspace. All Rust crates live under `crates/`:

| Directory | Cargo package | Responsibility |
| --- | --- | --- |
| `config` | `gitadel-config` | Configuration and command-line options |
| `core` | `gitadel-core` | Shared domain types and utilities |
| `db` | `gitadel-db` | Entities, migrations, and database access |
| `secrets` | `gitadel-secrets` | Secret storage and encryption |
| `mail` | `gitadel-mail` | Mail delivery |
| `storage` | `gitadel-storage` | Blob stores, LFS objects, registry persistence, and migrations |
| `backups` | `gitadel-backups` | Backup archives, providers, restore, and schedule persistence |
| `integrations` | `gitadel-integrations` | External provider clients and deployment operations |
| `actions` | `gitadel-actions` | Workflow planning, runner operations, and run persistence |
| `identity` | `gitadel-identity` | Authentication, account credentials, sessions, and permissions |
| `repositories` | `gitadel-repositories` | Git transport, imports, content, editing, and archive services |
| `api` | `gitadel-api` | HTTP handlers, protocol integration, and service orchestration |
| `server` | `gitadel` | Server executable `gitadel` |
| `cli` | `gitadel-cli` | Remote client executable `gtd` |

The default member is `crates/server`. Library packages use the `gitadel-`
prefix; directory names express their role. Keep workspace package metadata
and dependency versions in the root manifest. Library manifests explicitly
declare `[lib] path = "src/lib.rs"` so CLI packaging can include unselected
crates' manifests without their source trees.

## Development loop

REST changes must keep the handler-derived contract and generated frontend
bindings in sync. Register REST handlers through the existing `api_routes`
and `crate::openapi::routes!` helpers, use concrete JSON DTOs, then run
`just api-generate` and `just api-check`. The frontend build runs real type
checking before bundling; production Rust packaging requires its matching
fingerprint. See [API-CONTRACT.md](API-CONTRACT.md) for coverage and limits.

Enter the Nix dev shell, then run:

```bash
just dev
```

`just dev` starts Process Compose. It builds the frontend first, waits for
`frontend/build/index.html`, then starts Gitadel. Process Compose owns both
process groups, their logs, startup ordering, and shutdown.

Run one side on its own when needed:

```bash
just frontend
just backend
```

See every project command with `just --list`. Build, check, and test commands
also live in the root `Justfile`; `flake.nix` only installs their tools.

The frontend process uses `watchexec` over explicit input paths. Each change
launches a finite `bun run --cwd frontend build`, writes `frontend/build/`, and
exits. Do not replace it with `vite build --watch`. The production Vite watcher
retained build graphs in this project and was observed using more than 19 GiB.

The backend process is:

```bash
watchexec --restart --watch crates --watch Cargo.toml --exts rs,toml -- cargo run --package gitadel
```

Only the backend owns a port. In debug builds the backend reads
`frontend/build/` from disk on every request, so a completed frontend build is
served without restarting Rust. The backend restarts for Rust source and
workspace manifest changes.

Keep the frontend and backend on one origin. Do not use
`bun run --cwd frontend dev`; that server proxies only `/api` and `/healthz`,
so backend routes such as `/login/oauth/authorize` return its own 404.

A request that lands while the frontend builder rewrites the output may receive
a transient 503. Retry it. The browser may also retain the previous page, so
hard reload before deciding that a frontend change did not land.

## Configuration

Bare defaults are `127.0.0.1:3000` with SSH on `2222`, a `gitadel.db` in the
repository root, and `repositories/`, `lfs/`, `registry/`, and an SSH host key
beside it - all gitignored. Those ports collide with any Gitadel already running on the
machine, and the SSH listener failing is fatal:

```
Error: could not bind SSH listener to 127.0.0.1:2222
Caused by: Address already in use (os error 98)
```

So give an instance its own ports and paths. `gitadel.toml` in the repository
root is read automatically and is gitignored, which keeps the development
commands free of machine-specific flags:

```toml
[server]
bind = "0.0.0.0:3030"
public_url = "http://gitadel.example.com:3030"

[database]
url = "sqlite://data/gitadel.db?mode=rwc"

[storage]
repository_root = "data/repositories"
lfs_root = "data/lfs"
registry_root = "data/registry"

[ssh]
bind = "0.0.0.0:2222"
host_key = "data/ssh-host-ed25519"
```

Behind a reverse proxy add `trusted_proxies = ["10.0.0.0/8"]` under `[server]`
so rate limits and audit events see the real client, and `[audit]
retention_days = 365` caps the audit history; both default off. INSTALL.md
documents every option.

Any subset works; the rest falls back to the defaults. Per-run overrides use
the CLI flags or their `GITADEL_*` environment variables, which win over the
file. Bind `0.0.0.0` to reach the instance from other devices, and keep a
hostname in `public_url` - WebAuthn rejects bare-IP origins.
