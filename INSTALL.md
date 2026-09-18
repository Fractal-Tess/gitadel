# Install Gitadel

Gitadel can run from Docker Compose or as a NixOS service. In both cases, put internet-facing HTTP traffic behind a TLS-terminating reverse proxy. Gitadel serves HTTP and SSH directly but does not manage certificates.

## Docker Compose

Clone the repository and start the standalone server:

```bash
git clone https://github.com/Fractal-Tess/gitadel.git
cd gitadel
docker compose up -d --build
```

The default setup binds HTTP to `127.0.0.1:3000` and SSH to
`127.0.0.1:2222`. Check the container and its health endpoint:

```bash
docker compose ps
curl --fail http://127.0.0.1:3000/healthz
```

Open [http://localhost:3000/register](http://localhost:3000/register) and
create the first administrator. The `gitadel-data` named volume keeps the
database, repositories, LFS objects, and SSH host key when the container is
recreated.

The image runs Gitadel as the non-root user with UID and GID `10001`. Docker
creates the named volume with the image's `/data` ownership, so the normal
deployment does not need a permission fix. If you replace the named volume
with a host bind mount, make the mounted directory writable by that identity:

```bash
sudo chown -R 10001:10001 /path/to/gitadel-data
```

The runtime image includes Gitadel and its shared libraries, but does not
install a system `git` executable. Git operations are handled by Gitadel.

To stop and restart without removing data:

```bash
docker compose stop
docker compose start
```

`docker compose down` removes containers but keeps `gitadel-data`. Do not use
`docker compose down -v` for an instance that contains data. That command
deletes the named volume.

For access from another host, bind the published ports on all interfaces and
set the browser-visible URL. Put public HTTP traffic behind a
TLS-terminating reverse proxy:

```bash
GITADEL_LISTEN_ADDRESS=0.0.0.0 \
GITADEL_PUBLIC_URL=https://git.example.com \
GITADEL_HTTP_PORT=3000 \
GITADEL_SSH_PORT=2222 \
docker compose up -d --build
```

`GITADEL_PUBLIC_URL` defaults to `http://localhost:3000` and must be the
origin that users open in their browsers. Gitadel uses it for clone links,
cookies, passkey verification, OAuth callbacks, and webhook payloads. Include
the public HTTP port in the URL when it is not the standard port for its
scheme.

`GITADEL_HTTP_PORT` and `GITADEL_SSH_PORT` change the host ports. Gitadel
currently has no separate advertised SSH-port setting. Keep the default host
SSH port `2222` if you want generated SSH clone URLs to work as shown. If you
map SSH to another host port, edit the port in each SSH clone URL or Git
remote manually. The service still listens on port `2222` inside Compose.

Actions runners are optional. See [Actions deployment](docs/actions.md#docker-compose)
for the `compose.actions.yaml` overlay, which adds a privileged Docker-in-Docker
daemon and a Forgejo Runner.

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

`gtd` talks to a running Gitadel server over HTTP(S). It does not open the server's database or repository directories. Install it with `nix profile install github:Fractal-Tess/gitadel#gitadel-cli`, or build it from source with `cargo build --release -p gitadel-cli`. The executable is `gtd`; the Cargo package, Nix flake outputs, and NixOS options retain the name `gitadel-cli`.

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

JSON commands write JSON to stdout and errors to stderr, with a nonzero exit status on failure. `--body-file -` accepts JSON from stdin, but cannot share stdin with a token source. Typed commands cover repositories, organizations, SSH keys, instance and authentication settings, OIDC, storage, backups, and audit records; `gtd --help` lists them. The `api` command exposes other token-authorized endpoints. Browser-only account-security and restore flows keep their existing authentication requirements.

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
It needs no separate registry process, port, or data volume.

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
activity for that image.

By default, registry files live under
`repository_root/<storage-key>.git/gitadel-registry/`, outside Git's object
database. **Administration → Container registry** reports stored bytes,
blob and manifest counts, tags, images, and staged uploads. Its repository
list supports name, owner, owner type, and byte-range filters.

Choose a filesystem or S3-compatible target created under
**Administration → Storage**, or return to repository-backed local storage.
Git LFS and the registry can share a target but select their destinations
independently. Changing the LFS target does not move container images.
External registry payloads use a separate `registry/` prefix; tags, manifest
metadata, and upload staging stay beside the local bare repository.

Migration waits for in-flight write requests, then pauses registry mutations
while it copies and verifies payload sizes and SHA-256 content. Pulls remain
available. A failed or interrupted migration leaves the previous target
selected. Source copies are retained; selecting an old target again removes
payloads that were deleted from the current target before cutover.

Renaming or transferring a repository keeps its images. Soft deletion hides
them, and restoration makes them available again. Permanent purge removes
registry data from local storage and reachable configured targets.

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
manifest. Restore
rejects changed, missing, or extra files and only writes database and storage
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
