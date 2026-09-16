# Working on Gitadel

## Development loop

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
watchexec --restart --exts rs -- cargo run
```

Only the backend owns a port. In debug builds the backend reads
`frontend/build/` from disk on every request, so a completed frontend build is
served without restarting Rust. The backend restarts only for Rust changes.
Add `-w Cargo.toml` when editing dependencies.

Keep the frontend and backend on one origin. Do not use
`bun run --cwd frontend dev`; that server proxies only `/api` and `/healthz`,
so backend routes such as `/login/oauth/authorize` return its own 404.

A request that lands while the frontend builder rewrites the output may receive
a transient 503. Retry it. The browser may also retain the previous page, so
hard reload before deciding that a frontend change did not land.

## Configuration

Bare defaults are `127.0.0.1:3000` with SSH on `2222`, a `gitadel.db` in the
repository root, and `repositories/`, `lfs/`, and an SSH host key beside it -
all gitignored. Those ports collide with any Gitadel already running on the
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
public_url = "http://kiwi.netbird.cloud:3030"

[database]
url = "sqlite://data/gitadel.db?mode=rwc"

[storage]
repository_root = "data/repositories"
lfs_root = "data/lfs"

[ssh]
bind = "0.0.0.0:2222"
host_key = "data/ssh-host-ed25519"
```

Any subset works; the rest falls back to the defaults. Per-run overrides use
the CLI flags or their `GITADEL_*` environment variables, which win over the
file. Bind `0.0.0.0` to reach the instance from other devices, and keep a
hostname in `public_url` - WebAuthn rejects bare-IP origins.
