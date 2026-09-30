# Install Gitadel

Gitadel can run from Docker Compose or as a NixOS service. In both cases, put internet-facing HTTP traffic behind a TLS-terminating reverse proxy. Gitadel serves HTTP and SSH directly but does not manage certificates.

## Docker Compose

Images for `linux/amd64` and `linux/arm64` are published to
`ghcr.io/fractal-tess/gitadel`. The compose files pull them, so you only need
the file itself:

```bash
curl -fsSLO https://raw.githubusercontent.com/Fractal-Tess/gitadel/main/compose.yaml
docker compose up -d
docker compose logs gitadel
```

The log contains a one-time link to create the first administrator:

```text
No administrator exists yet. Open this one-time link to create one: http://localhost:3000/register?setup=...
```

Open it and choose a username and password. The link stops working once the
administrator exists, and a restart prints a fresh one until then. HTTP
listens on `127.0.0.1:3000`, SSH on `127.0.0.1:2222`, and the `gitadel-data`
volume keeps the database, repositories, LFS objects, Actions artifacts, and
SSH host key.

### Settings

Configure the stack with environment variables, either exported or in an
`.env` file next to the compose file:

| Variable | Default | Purpose |
| --- | --- | --- |
| `GITADEL_PUBLIC_URL` | `http://localhost:3000` | The address people open in a browser. Used for clone links, cookies, OAuth callbacks, and webhooks. |
| `GITADEL_LISTEN_ADDRESS` | `127.0.0.1` | Host address for the published ports. Use `0.0.0.0` to accept connections from other machines. |
| `GITADEL_HTTP_PORT` | `3000` | Host port for HTTP. |
| `GITADEL_SSH_PORT` | `2222` | Host port for Git over SSH. |
| `GITADEL_VERSION` | `latest` | Image tag, such as `0.22.0`. |

For example, to reach an instance on your network at `http://192.168.1.10:3000`:

```bash
GITADEL_LISTEN_ADDRESS=0.0.0.0 GITADEL_PUBLIC_URL=http://192.168.1.10:3000 docker compose up -d
```

Passkeys need a hostname in `GITADEL_PUBLIC_URL`, so they are turned off for
IP addresses; passwords and TOTP still work. Put public traffic behind a
TLS-terminating reverse proxy and use its `https://` address as the public
URL. SSH clone URLs always show port `2222`; if you publish SSH on another
host port, adjust the remote by hand.

### Hosting platforms

Coolify, CapRover, Dokploy, and Portainer can deploy `compose.yaml` or
`compose.actions.yaml` as a single stack. Set `GITADEL_PUBLIC_URL` to the
domain the platform assigns, route that domain to container port `3000`,
optionally expose port `2222` for SSH, and open the setup link from the
service log.

### Actions runner

`compose.actions.yaml` is the same stack plus a Forgejo Runner and the
privileged Docker daemon that runs its jobs. It needs no other files:

```bash
curl -fsSLO https://raw.githubusercontent.com/Fractal-Tess/gitadel/main/compose.actions.yaml
docker compose -f compose.actions.yaml up -d
docker compose -f compose.actions.yaml logs gitadel   # setup link
```

The runner registers itself as the instance-wide `gitadel-system` runner with
the `docker` label, using a one-time token Gitadel writes to a shared volume.
Jobs reach Gitadel at `http://gitadel:3000` inside the stack, so checkouts,
artifacts, and `actions/cache` work whatever the public URL is.

Workflows in `.forgejo/workflows`, `.gitea/workflows`, or `.github/workflows`
run with `runs-on: docker` on push, `workflow_dispatch`, or `schedule` (UTC
cron on the default branch), in the pinned `node` image. Remote actions must
be pinned to a full commit and come from `https://code.forgejo.org`:

```yaml
steps:
  - uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683 # v4.2.2
  - run: make test
```

The Gitadel and runner settings are inline in the `configs` section of
`compose.actions.yaml`. Remove `allowed_action_origins` there to permit only
`run:` steps and local `./` actions. The job daemon is privileged; run only
workflows you trust.

### Operating the stack

```bash
docker compose ps                  # health
docker compose pull && docker compose up -d   # upgrade
docker compose stop                # stop without removing data
```

`docker compose down` keeps the `gitadel-data` volume; `docker compose down -v`
deletes it along with every repository. The container runs as UID and GID
`10001`. If you replace the named volume with a bind mount, make the directory
writable by that user:

```bash
sudo chown -R 10001:10001 /path/to/gitadel-data
```

To create the administrator from the command line instead of the setup link,
stop the server and run:

```bash
docker compose stop gitadel
read -rsp "Password: " password; echo
printf '%s' "$password" | docker compose run --rm -T gitadel --bootstrap-admin admin --password-stdin
docker compose start gitadel
```

To build the image from a checkout instead of pulling it, add
`compose.build.yaml`:

```bash
docker compose -f compose.yaml -f compose.build.yaml up -d --build
```

### Publishing the image

Pushing a `vX.Y.Z` tag to GitHub runs `.github/workflows/release-image.yml`.
It builds `linux/amd64` and `linux/arm64` images on native runners, publishes
them as `X.Y.Z`, `X.Y`, and `latest` to `ghcr.io/fractal-tess/gitadel`, and
creates a GitHub release from the matching `CHANGELOG.md` section.

To publish by hand, including to the Gitadel registry on the private mesh,
`scripts/publish-image.sh` builds the current checkout once, then publishes
each requested tag to these repositories:

- `ghcr.io/fractal-tess/gitadel`
- `neo.netbird.cloud:3030/fractal-tess/gitadel`
- `docker.io/vgfractal/gitadel`

It requires Docker and Skopeo and reuses your Docker logins. When `gh` is
installed, it supplies the GHCR token. Authorize package writes once with
`gh auth refresh --hostname github.com --scopes write:packages`. Otherwise,
use `docker login ghcr.io` with a token that has `write:packages`.
Use `docker login --username vgfractal` for Docker Hub.

On a machine with the Gitadel SOPS token and private mesh access:

```bash
GITADEL_USERNAME=fractal-tess \
GITADEL_TOKEN_FILE=/run/secrets/gitadel_api_token \
GITADEL_TLS_VERIFY=false \
nix shell nixpkgs#skopeo --command ./scripts/publish-image.sh 0.22.0 latest
```

With Skopeo already installed, `just publish-image 0.22.0 latest` calls the
same script; keep the environment variables above. No tag argument means
`latest`. The build targets the Docker daemon's native platform, not a
multi-platform image.

Only use `GITADEL_TLS_VERIFY=false` for the HTTP registry on the private
mesh. It does not change Docker's daemon settings or TLS verification for
GHCR and Docker Hub. Gitadel tokens need `read` and `write` scopes.
Automatically acquired credentials are stored in a temporary file and
removed when the script exits.

Override destinations with `GHCR_IMAGE`, `GITADEL_IMAGE`, and
`DOCKERHUB_IMAGE`; values are repository paths without tags or URL schemes.
`LOCAL_IMAGE` changes the local build tag. The script prints each published
digest, continues to the remaining destinations if a push fails, and exits
nonzero if any push or verification failed. Successful pushes are not
rolled back; rerunning reuses Docker's build cache and existing layers.

## NixOS

Add the flake and enable its module:

```nix
{
  inputs.gitadel.url = "github:Fractal-Tess/gitadel";

  outputs = { nixpkgs, gitadel, ... }: {
    nixosConfigurations.archive = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        gitadel.nixosModules.default
        {
          services.gitadel = {
            enable = true;
            initialAdmin = {
              username = "admin";
              passwordFile = "/run/secrets/gitadel-initial-admin";
            };
            autoStart = true;
            publicUrl = "https://git.example.com";
            openFirewall = true;
          };
        }
      ];
    };
  };
}
```

The module runs Gitadel as a hardened systemd service and stores persistent state in `/var/lib/gitadel`. `services.gitadel.package` defaults to the server package from the pinned flake and only needs to be set to override it; the module does not apply a consumer overlay.

`services.gitadel.autoStart` defaults to `true`, so the Gitadel unit and, when enabled, both Gitadel-owned runner units are wanted by `multi-user.target`. Set it to `false` to keep those units and their manual startup dependencies without booting them automatically. This option does not change the lifecycle of the shared host `docker.service`.

```nix
services.gitadel = {
  enable = true;
  autoStart = false;
  runner.enable = true;
};
```

Starting `gitadel-runner` manually still starts `gitadel-runner-docker` and `gitadel` through their existing `requires` edges. The host Docker service remains governed by the NixOS Docker module and any other services that use it.

Ports, storage, and authentication lifetimes have dedicated options. Additional TOML values belong in `settings`, which is merged over the generated configuration:

```nix
services.gitadel = {
  enable = true;
  publicUrl = "https://git.example.com";
  http = { address = "0.0.0.0"; port = 3000; };
  ssh = { address = "0.0.0.0"; port = 2222; };
  dataDir = "/var/lib/gitadel";
  database.url = "sqlite:///var/lib/gitadel/gitadel.db?mode=rwc";
  auth = {
    sessionLifetimeHours = 24 * 30;
    invitationLifetimeHours = 72;
  };
  settings = { };
};
```

The service receives `CAP_NET_BIND_SERVICE` automatically when either listener uses a port below `1024`.

### Secrets

The generated TOML is stored in the world-readable Nix store. Put secrets in a systemd environment file instead:

```nix
services.gitadel = {
  environment = {
    "GITADEL__SERVER__PUBLIC_URL" = "https://git.example.com";
  };
  environmentFile = "/run/secrets/gitadel.env";
};
```

Use `environment` for non-secret runtime variables and `environmentFile` for credentials:

```ini
# /run/secrets/gitadel.env
GITADEL__DATABASE__URL=postgres://gitadel:secret@localhost/gitadel
```

Environment variables use `GITADEL__SECTION__KEY` names and override TOML settings.

### Initial administrator

`initialAdmin` creates the first administrator before the service starts:

```nix
services.gitadel.initialAdmin = {
  username = "archivist";
  passwordFile = "/run/secrets/gitadel-admin-password";
};
```

Bootstrapping becomes a no-op after any account exists, so the option can remain configured. The password file must be readable by the service user and cannot live under `/home` or `/root`, which the unit hides.

### Package and module alternatives

The server and remote client have separate packages and modules:

| Purpose | Flake output |
| --- | --- |
| Server package and app | `gitadel` (`default` also selects the server) |
| Remote client package and app | `gitadel-cli` (runs `gtd`) |
| Server NixOS module | `nixosModules.gitadel` or `nixosModules.default` |
| Client NixOS module | `nixosModules.gitadel-cli` |

Install the client on a machine without enabling the server:

```nix
{
  imports = [ inputs.gitadel.nixosModules.gitadel-cli ];
  programs.gitadel-cli = {
    enable = true;
    serverUrl = "https://git.example.com";
  };
}
```

`serverUrl` supplies a default `GITADEL_SERVER` through the installed executable's wrapper, not a session-wide environment variable. Inherited environment values and explicit CLI flags can override it. Do not put an API token in Nix configuration or the Nix store. Override `programs.gitadel-cli.package` or `services.gitadel.package` to select another build.

## Command-line client

`gtd` talks to a running Gitadel server over HTTP(S). It does not open the server's database or repository directories. Download a prebuilt binary for Linux (x86_64 or arm64, statically linked), macOS, or Windows (x86_64, `.zip`) from the [latest release](https://github.com/Fractal-Tess/gitadel/releases/latest):

```bash
curl -fsSL https://github.com/Fractal-Tess/gitadel/releases/latest/download/gtd-VERSION-x86_64-unknown-linux-musl.tar.gz | tar -xz
sudo install gtd /usr/local/bin/
```

Replace `VERSION` with the release number shown on the page. You can also install it with `nix profile install github:Fractal-Tess/gitadel#gitadel-cli`, or build it with `cargo build --release -p gitadel-cli`. The executable is `gtd`; the Cargo package, Nix flake outputs, and NixOS options retain the name `gitadel-cli`.

Create an API token under **Account settings → Access**. Reads require `read`; repository, organization, and administrator mutations require `write`. Adding or removing your SSH keys requires `ssh_keys`. Administrator commands also require an administrator account. A token does not bypass repository or organization permissions.

### Login on Linux, WSL, and other systems

No NixOS service or desktop keyring is required. From a source checkout with the Rust toolchain installed, `cargo install --path cli --locked` installs the client on your Cargo binary path.

```bash
gtd auth login
gtd me profile
gtd repo list
gtd auth status
```

Login prompts for the server origin and an API token without echoing the token. It checks the token with the server before replacing the saved login. You can provide the endpoint explicitly with `gtd --server https://git.example.com auth login`.

The client remembers one login in the platform's per-user configuration directory. On Linux and WSL this is `$XDG_CONFIG_HOME/gitadel/auth.json`, normally `~/.config/gitadel/auth.json`. The file stores the token **unencrypted**; on Unix the directory is mode `0700` and the file is mode `0600`. Do not sync it into a public dotfiles repository. Login output identifies the storage location.

`gtd auth logout` removes the saved login. It does not revoke the token on the server, delete an external token file, or disable environment credentials. Revoke tokens under **Account settings → Access**.

### NixOS and SOPS

Configure your encrypted secret through sops-nix as usual, then pass only its runtime path to the CLI module:

```nix
{ config, inputs, ... }: {
  imports = [ inputs.gitadel.nixosModules.gitadel-cli ];

  sops.secrets.gitadel_api_token = {
    owner = "alice"; # The user running gtd.
    mode = "0600";
  };

  programs.gitadel-cli = {
    enable = true;
    serverUrl = "https://git.example.com";
    tokenFile = config.sops.secrets.gitadel_api_token.path;
  };
}
```

After installing the configuration, run `gtd me profile`; no login command or new shell session is needed. The wrapper supplies `GITADEL_SERVER` and `GITADEL_TOKEN_FILE` defaults. The client reads the secret file on each invocation, so secret rotation does not require rebuilding the wrapper. Neither the token contents nor a build-time read of the secret enters the Nix store. The decrypted file must be readable by the user running the CLI.

### Token sources and automation

Without NixOS, `GITADEL_TOKEN_FILE` provides the same runtime-file behavior. To remember a token-file reference without environment variables:

```bash
gtd --server https://git.example.com --token-file /path/to/token auth login
gtd me profile
```

This saves the file path, not its contents, and preserves symlinks such as rotating SOPS paths. The token file contains only the token. Explicit `--token-stdin` or `--token-file -` reads from stdin; using either with `auth login` stores the token itself.

Server selection is `--server`, then `GITADEL_SERVER`, then the saved server, then `http://127.0.0.1:3000`. Token precedence is an explicit token flag, then `GITADEL_TOKEN`, then `GITADEL_TOKEN_FILE`, then the saved login. Saved credentials are used only for their matching origin. Environment credentials paired with `GITADEL_SERVER` are also withheld when `--server` selects another origin; an explicit token flag is a deliberate override.

Explicit token sources are mutually exclusive. The environment token-file path is always a file, not stdin. `--token` remains supported, but exposes the value in process arguments; prefer hidden login entry, a file, or stdin.

For CI, provide `GITADEL_SERVER` and a masked `GITADEL_TOKEN` secret through the job environment:

```bash
gtd repo list
gtd admin instance update --body-file instance-settings.json
gtd api user
```

JSON commands write JSON to stdout and errors to stderr, with a nonzero exit status on failure. `--body-file -` accepts JSON from stdin, but cannot share stdin with a token source. Typed commands cover repositories, issues, releases and their assets, webhooks, pull mirrors and mirror identities, repository imports, container registry images and storage, organizations, SSH keys, instance and authentication settings, OIDC, storage, backups, and audit records; `gtd --help` lists them. The `api` command exposes other token-authorized endpoints. API tokens and backup restores require a browser session, or `gitadel backup restore` on the server host.

```bash
gtd issue create alice/demo --title "Crash on start" --body-file notes.md --label bug
gtd release create alice/demo --target v1.2.0 --body-file CHANGES.md
gtd release asset upload alice/demo v1.2.0 dist/demo.tar.gz
gtd webhook create alice/demo --url https://ci.example.com/hook --secret-file hook.secret
```

Commands accept an ID or a readable name: a label name, a release tag or title, or an asset file name.

Backup downloads stream into a private temporary file and replace the requested output only after a complete transfer:

```bash
gtd admin backup providers list
gtd admin backup download "$PROVIDER_ID" "$BACKUP_KEY" --output backup.tar.zst
gtd admin backup progress "$OPERATION_ID"
```

Progress commands emit JSON Lines, reconnect after an interrupted stream, and exit unsuccessfully when the operation fails. Use HTTPS outside an encrypted private network; bearer tokens grant the account's configured access.

## Configuration

When running the binary directly, Gitadel reads `gitadel.toml` from the working directory if it exists. Configuration precedence is:

```text
command line > environment > TOML > defaults
```

Nested environment keys use double underscores, for example:

```bash
GITADEL__SERVER__BIND=0.0.0.0:3000
GITADEL__SSH__BIND=0.0.0.0:2222
```

Common command-line options also have aliases including `GITADEL_CONFIG`, `GITADEL_BIND`, `GITADEL_PUBLIC_URL`, and `GITADEL_DATABASE_URL`. Run `gitadel --help` for the complete list and defaults.

### Git over HTTP

Gitadel serves the Smart HTTP protocol on the same origin as the web interface. Use the HTTPS clone URL shown on a repository page:

```bash
git clone https://git.example.com/owner/repository.git
git push origin main
```

For a private clone or any push, enter your Gitadel username and an API token when Git asks for a password. A clone needs the token's `read` scope; pushing needs `write` as well. Create tokens under **Account settings → Access**. Do not put a token directly in a remote URL because Git stores that URL in `.git/config`.

Plain HTTP also works, but sends the credential without application-layer encryption. Keep it on an encrypted private network such as NetBird; use HTTPS anywhere else.

### Default branches

An empty or tags-only repository has no default branch (`null` in the API). On the first branch upload, Gitadel preserves a valid configured branch or chooses `main`, `master`, `prod`, then `staging`. If none exists, it chooses the branch with the newest committer timestamp at its tip, breaking ties by branch name. Initial imports and mirrors prefer the remote's advertised HEAD.

The choice is made before repository analysis and written to the bare repository's symbolic HEAD. Later pushes leave it unchanged. Maintainers can choose another existing branch under **Repository settings → General**.

### Images and repository icons

The file browser previews SVG, PNG, JPEG, WebP, GIF, AVIF, BMP, and ICO files. Animated images show their first frame. Previews are limited to 16 MiB of source, 16,777,216 pixels, and 16,384 pixels per edge. A time- and memory-limited subprocess renders PNG output; SVG previews cannot fetch external resources. Oversized or unsupported files still offer the original download, and raw SVG files download as attachments. LFS images use the same repository access checks as other files.

**Repository settings → General → Icon** lists logos detected in the repository. Detection considers branding names, README references, asset locations, and image dimensions. Startup scans existing repositories in the background; pushes and LFS uploads refresh the candidates. Scans have bounded work limits, and a partial or failed scan preserves an existing automatic icon.

Choose a candidate to follow that path on the default branch, upload an image, remove the icon, or return to automatic selection. Uploads and the no-icon preference survive later pushes. If a selected file disappears, Gitadel keeps its last valid image and marks the path as missing.

### Repository integrity checks

Gitadel checks every active repository once a day at 03:00 UTC by default. Native checks verify Git objects, references, pack storage, indexes, and commit graphs; verify LFS objects against their SHA-256 IDs; check release and issue-attachment files; and reject database records that point outside their storage roots. The result appears in the administrator activity log. A failed check is also written to the server log with the affected repository and error. No external `git fsck` process is required.

Administrators can enable or disable the job and edit its UTC cron expression under **Administration → Maintenance**. Five-, six-, and seven-field cron expressions are accepted. The page also shows the timestamp and result of the last completed pass.

### Email (SMTP)

Outgoing email is optional. Without an `[smtp]` section, password reset,
email verification, and email settings are hidden. To enable them:

```toml
[smtp]
host = "smtp.example.com"
# port defaults to 587 for starttls, 465 for tls, and 25 for none.
port = 587
# starttls (default), tls (implicit TLS), or none (trusted local relay only).
tls = "starttls"
username = "gitadel@example.com"
# Set password or password_file, not both. The file's trailing newline is ignored.
password_file = "/run/secrets/gitadel-smtp-password"
from = "Gitadel <gitadel@example.com>"
```

`username` and a password are set together or not at all. Each key has a
`GITADEL__SMTP__*` environment variable, for example `GITADEL__SMTP__HOST` and
`GITADEL__SMTP__PASSWORD_FILE`. Invalid settings or an unreadable password file
stop startup; an unreachable server does not.

Mail is queued and sent by a background task, so requests never wait for the
server. Check the configuration under **Administration → Email**, which sends a
test message and shows the server's answer, or with `gtd admin smtp test --to
you@example.com`.

Users add an address under **Account settings → Profile** (or `gtd me email set
you@example.com`) and confirm it through an emailed link, valid for 24 hours.
Only verified addresses receive mail. The sign-in page then offers **Forgot
password?**: a reset link is sent to the account's verified address, expires
after an hour, and works once. The request form answers the same way whether or
not an account exists, and completing a reset signs out every session.

Verified addresses also receive notifications: new issues in repositories the
user owns (personally or as an organization owner), issues assigned to them,
comments on issues they own, opened, or are assigned to, and failed Actions runs
they triggered. Nobody is emailed about their own actions or about repositories
they can no longer read. Each kind can be switched off under **Account settings
→ Profile → Notifications** or with `gtd me notifications set --issues false`.

### Local HTTPS for passkeys

Passkeys require a secure browser origin. Gitadel can terminate HTTPS directly
when a PEM certificate chain and private key are configured:

```toml
[server]
bind = "0.0.0.0:3030"
public_url = "https://kiwi.netbird.cloud:3030"

[server.tls]
certificate = "data/tls/gitadel.pem"
private_key = "data/tls/gitadel-key.pem"
```

The development shell includes `mkcert` and the NSS `certutil` tool. Generate a
certificate for the names used by your browsers:

```bash
mkdir -p data/tls
mkcert -cert-file data/tls/gitadel.pem \
  -key-file data/tls/gitadel-key.pem \
  kiwi.netbird.cloud localhost 127.0.0.1 ::1
```

`mkcert -install` configures trust automatically on supported systems. On
NixOS, import the generated CA into Chromium's user NSS database instead:

```bash
certutil -A -d "sql:$HOME/.pki/nssdb" -t "C,," \
  -n "mkcert development CA" -i "$(mkcert -CAROOT)/rootCA.pem"
```

Restart the browser after changing its trust database. Every other device that
opens the NetBird URL must also trust `$(mkcert -CAROOT)/rootCA.pem`; never
copy `rootCA-key.pem` or the Gitadel private key to another device.

## Container registry

Gitadel includes a Docker/OCI registry at `/v2/` on its existing HTTP origin.
It needs no separate registry process or port.

Create a lowercase Git repository such as `archivist/my-app` first, through
the web UI or `gtd repo create`. Under **Account settings → API tokens**, create
an API token with `read` and `write` scopes. Use that token as the password
for Docker login, not your account password:

```bash
docker login git.example.com --username archivist
docker tag my-app:latest git.example.com/archivist/my-app:latest
docker push git.example.com/archivist/my-app:latest
docker pull git.example.com/archivist/my-app:latest
```

For automation, use `docker login --password-stdin` with a secret supplied
by your CI or secret manager. Images can also have a suffix, such as
`git.example.com/archivist/my-app/worker:latest`; they still belong to
`archivist/my-app`. Organization images use the organization namespace,
but login always uses your own Gitadel username.

Open **Container registry** in the repository sidebar to browse its images,
tags, and digest-only references, or copy a Docker pull command. The tab remains
visible when empty and shows publishing instructions to repository writers.
Stored sizes count blob and manifest payloads once, regardless of how many tags
reference them, and exclude unfinished uploads. New pushes record timestamps;
older references show unavailable dates until pushed again.

`GET /api/v1/repositories/{namespace}/{name}/registry` returns the same listing.
It accepts browser sessions or API tokens with `read` scope, and applies the
repository's normal visibility and access checks.

Public repositories allow anonymous pulls. Private images require the
same repository access as Git; pulls need a `read` token, and Docker pushes
need `read` and `write`. Deleting registry content additionally requires
repository management permission. Archived and mirrored repositories
reject mutations. Each request rechecks the source token and current
repository permissions, so revocation and access changes also apply to
previously issued registry tokens.

Use HTTPS for remote Docker clients. If you deliberately use plain HTTP
outside loopback, Docker must be configured to trust that host and port
as an insecure registry. Keep that traffic on an encrypted private
network. The reverse proxy must forward `/v2/`, preserve authorization
headers, allow streaming uploads and sufficiently large request bodies,
and leave registry responses uncompressed.

The registry supports Docker schema 2 manifests and manifest lists, OCI
image manifests and indexes, resumable uploads, cross-image layer mounts,
tag/catalog pagination, referrers, and tag/manifest/blob deletion through
the OCI Distribution API. `docker image rm` only removes a local copy.
Deleting a registry tag leaves its digest and layers intact; a manifest
referenced by an index cannot be deleted until that index is removed.
Unreferenced data is not garbage-collected automatically.

Blobs use SHA-256 and are limited to 10 GiB each; manifests are limited to
4 MiB. External descriptor URLs are not supported. Upload sessions expire
after 24 hours; expired sessions are cleaned up during later upload
activity for that repository.

Tags, manifest records, and image names live in the database. Blobs and
manifest bytes are content-addressed payloads, shared by the images of one
repository, in the selected registry storage. By default that is the local
registry root, `storage.registry_root` (`registry` relative to the working
directory; `--registry-root` or `GITADEL_REGISTRY_ROOT`; `/data/registry` in
the container image and `<dataDir>/registry` with the NixOS module), which
holds one directory per repository storage key. Resumable upload sessions
are always staged locally under `registry_root/uploads/` and published to
the selected storage once their digest is verified. A layer mount between
images of one repository only records metadata; a mount from another
repository copies the payload, as a hard link on a filesystem.

Versions up to 0.13 kept registry files inside each bare repository. On its
first start, Gitadel moves them to the registry root, using a rename when
both are on one filesystem and otherwise a copy that is verified by size and
SHA-256 before the source is removed. It then imports tags and manifest
metadata into the database and rewrites payload keys in the selected
storage. Both steps log what they moved, resume safely after an
interruption, and run once. Unfinished upload sessions from before the
upgrade are discarded.

**Administration → Container registry** reports stored bytes,
blob and manifest counts, tags, images, and staged uploads. Its repository
list supports name, owner, owner type, and byte-range filters.

Choose a filesystem or S3-compatible target created under
**Administration → Storage**, or return to the local registry root. Because
metadata lives in the database, the whole registry can live on either kind of
target. Git LFS and the registry can share a target but select their
destinations independently. Changing the LFS target does not move container
images. Registry payloads use a separate `registry/` prefix on targets.

Migration waits for in-flight write requests, then pauses registry mutations
while it copies and verifies payload sizes and SHA-256 content. Pulls remain
available. A failed or interrupted migration leaves the previous target
selected. Source copies are retained; selecting an old target again removes
payloads that were deleted from the current target before cutover.

Renaming or transferring a repository keeps its images. Deleting a repository
is permanent and removes its registry data from local storage and reachable
configured targets.

Backups include the registry root and the registry tables. Payloads on a
storage target are copied into the backup, so a restored instance starts on
the local registry root. Backups made by 0.13 restore as well; the next start
converts their registry data as described above.

## Git LFS storage

Git repositories, issue attachments, and release assets remain under the configured local storage roots. Administrators define tested filesystem and S3-compatible destinations under **Administration → Storage**. User-defined storage targets are also available as backup destinations; Gitadel does not create a default backup provider. Filesystem backups use a sibling `<storage-name>-backups` directory so an archive never contains itself; S3 backups use a `backups` child of the target prefix.

Filesystem targets need write access to both the target directory and its
backup sibling. For example, `/mnt/archive/gitadel` uses
`/mnt/archive/gitadel-backups`. Create both with the Gitadel service user's
ownership. With a systemd filesystem sandbox, include both directories in
`ReadWritePaths`; allowing the target alone does not make its sibling writable.

**Administration → Git LFS** shows object counts and logical space by repository and owner. Search repository names, filter by user or organization and byte range, and sort by name or space. The list starts with ten repositories; **Load more** fetches the next ten.

Changing the active target runs in the background without restarting Gitadel. Browsing, Git operations, and LFS downloads remain available. LFS uploads continue during the initial copy, then wait while Gitadel drains in-flight writes, copies the remaining objects, and commits the target change. Migration verifies object size and SHA-256 content. Source data is retained; a failed or interrupted migration leaves the source active and can be retried.

The server's offline `gitadel lfs` migration commands still require Gitadel to be stopped. They are separate from `gtd`, which manages the running instance:

```bash
gitadel lfs target add-filesystem --name archive --path /mnt/archive/gitadel-lfs
gitadel lfs target list
gitadel lfs migrate <target-id>
```

For S3-compatible storage, pass the endpoint, bucket, region, and prefix as options. Supply credentials through the environment so they do not enter shell history:

```bash
export GITADEL_LFS_S3_ACCESS_KEY=access-key
export GITADEL_LFS_S3_SECRET_KEY=secret-key
gitadel lfs target add-s3 \
  --name object-storage \
  --endpoint https://s3.example.com \
  --bucket gitadel \
  --region us-east-1 \
  --prefix lfs
```

The target must be empty or contain Gitadel's matching ownership marker. S3 committed-object metadata is verified before reads, listings, migration cutover, and backup creation. Restores always materialize LFS data into the running instance's configured local `lfs_root`; restored database-backed targets remain inactive until an administrator migrates to one explicitly.

### Storage domains API and CLI

Git LFS (`lfs`) and the container registry (`registry`) are storage domains
that share one administrator API. Both Administration pages use it, and every
endpoint requires an administrator:

| Endpoint | Purpose |
| --- | --- |
| `GET /api/v1/admin/storage/domains` | Every domain with its active target or local root, usage totals, and active and last migration |
| `GET /api/v1/admin/storage/domains/{domain}` | One domain's status |
| `GET /api/v1/admin/storage/domains/{domain}/repositories` | Per-repository usage; accepts `search`, `owner`, `owner_type` (`user` or `organization`), `min_bytes`, `max_bytes`, `sort` (`bytes_desc`, `bytes_asc`, `name`), `limit` (1-100, default 10), and `offset` |
| `POST /api/v1/admin/storage/domains/{domain}/migrate` | Start an online migration; send `{"target_id": "..."}` or `{"local": true}`, with an optional `batch_size` |
| `GET /api/v1/admin/storage/domains/{domain}/migrations/{operation_id}` | Migration progress as JSON |
| `GET /api/v1/admin/storage/domains/{domain}/migrations/{operation_id}/events` | Migration progress as server-sent events |

An unknown domain returns 404. Domain-specific counters, such as registry
tags and staged uploads, appear under `details` and are described by the
status's `detail_fields`.

```bash
gtd admin storage domain list
gtd admin storage domain status registry
gtd admin storage domain repositories lfs --owner-type organization --min-bytes 1048576
gtd admin storage domain migrate registry --target "$TARGET_ID"
gtd admin storage domain migrate lfs --local
gtd admin storage domain progress registry "$OPERATION_ID"
```

`progress` streams JSON Lines until the migration finishes; add `--once` to
print the current state.

The per-domain routes `/api/v1/admin/storage/lfs/status`,
`/api/v1/admin/storage/lfs/repositories`, `/api/v1/admin/storage/migrations`,
`/api/v1/admin/storage/progress/{operation_id}` (outside maintenance mode), and
`/api/v1/admin/storage/registry/{status,repositories,migrate,migrations/{operation_id}/events}`
are deprecated aliases of the domain API and will be removed in a later
release. The `gtd admin storage lfs-status`, `lfs-repositories`, `migrate`,
and `progress` commands and `gtd admin registry` now call the domain API and
are deprecated as well.

## Backups

Offline backups made with the `gitadel` server binary use a storage lock to
guarantee one consistent snapshot. Stop the service before creating one. The
container runs as UID `10001`, so make a bind-mounted backup directory
writable by that UID:

```bash
mkdir -p backups
sudo chown 10001:10001 backups
docker compose stop gitadel
docker compose run --rm -v "$PWD/backups:/backups" \
  gitadel backup create /backups/gitadel-backup.tar.zst
docker compose start gitadel
```

The archive contains the SQLite database (users, instance settings,
credentials, and all other relational state), repositories and their
attachments and container images, LFS and release assets, the SSH host key,
and the effective Gitadel configuration. Every file is covered by a SHA-256
manifest.

Treat every backup archive as a secret-bearing credential bundle. Gitadel creates
local backup files with mode `0600`; keep their parent directory private, encrypt
archives at rest, and restrict access to the service operator. For remote backups,
use provider-side encryption with a dedicated key and a least-privilege bucket
policy in addition to HTTPS in transit.

Restore rejects changed, missing, or extra files and only writes database and storage
data into empty configured paths. Stop the service before restoring:

```bash
docker compose stop gitadel
docker compose run --rm -v "$PWD/backups:/backups:ro" \
  gitadel backup restore /backups/gitadel-backup.tar.zst
docker compose start gitadel
```

Registry and LFS payloads are restored into local storage, even when the
snapshot was created from an external target. The original object store is
not needed to serve restored data. Registry tags, referrers, and unfinished
uploads are included; stale payloads retained at an inactive target are not.

Offline restore also restores `gitadel.toml`. When relocating an instance,
update its database URL, storage paths, SSH host-key path, and listener
settings before starting it.

For an S3-compatible store, configure its API origin and bucket. The endpoint must be the S3 API origin, not a web-console URL:

```toml
[backup.s3]
endpoint = "https://s3.example.com"
bucket = "gitadel"
region = "us-east-1"
prefix = "backups"
```

Supply credentials through the environment when the configuration file is not secret:

```ini
GITADEL__BACKUP__S3__ACCESS_KEY=access-key
GITADEL__BACKUP__S3__SECRET_KEY=secret-key
```

While Gitadel is running, administrators manage backup providers under **Settings → Backups**. The catalog supports filesystem directories on the Gitadel host and S3-compatible storage. A destination must pass a write test before it can be saved, and each provider has its own automatic schedule. An existing `[backup.s3]` entry appears as a managed provider; editing or scheduling it stores a database-backed copy. The page lists snapshots for the selected provider, creates a current-instance backup, downloads an archive, deletes a snapshot after confirmation, and downloads and integrity-checks a selected snapshot before restore. Automatic backups can run hourly, every six hours, daily, weekly, or on a custom five-, six-, or seven-field UTC cron expression. The form translates custom cron into plain language and shows the next run. Gitadel briefly stops its normal HTTP, SSH, and mirror services for the actual backup or replacement. A restricted maintenance endpoint keeps the page updated with the snapshot, compression, and upload phase, including byte progress during S3 transfer, until the full server starts again automatically.

Online restore requires the administrator's current password and an explicit destructive-action acknowledgment. By default it first creates and verifies a safety backup of the current instance; if that backup fails, restore does not begin. The restored instance retains the running installation's bind addresses, storage paths, and selected backup provider so it can restart on the same host.

Then stop Gitadel and create a backup. The command prints the unique object key:

```bash
gitadel backup create-s3
gitadel backup restore-s3 backups/gitadel-20260827T120000Z-example.tar.zst
```

`backup create-s3 --key custom/path.tar.zst` uses an explicit key and refuses to replace an existing object. S3 uploads use multipart transfer for large archives and are verified against the stored object size. Backup archives contain credentials and password hashes; restrict bucket access and use HTTPS outside a trusted private network.
