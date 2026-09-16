# Install Gitadel

Gitadel can run from Docker Compose or as a NixOS service. In both cases, put internet-facing HTTP traffic behind a TLS-terminating reverse proxy. Gitadel serves HTTP and SSH directly but does not manage certificates.

## Docker Compose

Clone the repository and start the service:

```bash
git clone https://github.com/Fractal-Tess/gitadel.git
cd gitadel
docker compose up --build
```

Open [http://localhost:3000/register](http://localhost:3000/register) to create the first administrator. The default deployment exposes HTTP on `3000`, SSH on `2222`, and stores the database, repositories, LFS objects, and SSH host key in the `gitadel-data` volume.

Set the public URL or host ports through Compose environment variables:

```bash
GITADEL_PUBLIC_URL=https://git.example.com \
GITADEL_HTTP_PORT=3000 \
GITADEL_SSH_PORT=2222 \
docker compose up -d --build
```

`GITADEL_PUBLIC_URL` must be the browser-visible origin. Gitadel uses it for clone links, cookies, passkey verification, OAuth callbacks, and webhook payloads.

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
| Remote client package and app | `gitadel-cli` |
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

`serverUrl` sets `GITADEL_SERVER`; do not put an API token in Nix configuration or the Nix store. Override `programs.gitadel-cli.package` or `services.gitadel.package` to select another build.

## Command-line client

`gitadel-cli` talks to a running Gitadel server over HTTP(S). It does not open the server's database or repository directories. Install it with `nix profile install github:Fractal-Tess/gitadel#gitadel-cli`, or build it from source with `cargo build --release -p gitadel-cli`.

Create an API token under **Account settings → Access**. Reads require `read`; repository, organization, and administrator mutations require `write`. Adding or removing your SSH keys requires `ssh_keys`. Administrator commands also require an administrator account. A token does not bypass repository or organization permissions.

```bash
export GITADEL_SERVER=https://git.example.com
gitadel-cli --token-file ~/.config/gitadel/token me profile
gitadel-cli --token-file ~/.config/gitadel/token repo create archivist/old-project --private
gitadel-cli --token-file ~/.config/gitadel/token repo archive archivist/old-project
gitadel-cli --token-file ~/.config/gitadel/token admin instance get
```

The token file contains only the token and should have mode `0600`. `--token-stdin` and `--token-file -` read a token from standard input. An explicit token source overrides `GITADEL_TOKEN`; explicit sources are mutually exclusive. `--token` is also supported, but exposes the value in process arguments.

For CI, provide `GITADEL_SERVER` and a masked `GITADEL_TOKEN` secret through the job environment:

```bash
gitadel-cli repo list
gitadel-cli admin instance update --body-file instance-settings.json
gitadel-cli api user
```

JSON commands write JSON to stdout and errors to stderr, with a nonzero exit status on failure. `--body-file -` accepts JSON from stdin, but cannot share stdin with a token source. Typed commands cover repositories, organizations, SSH keys, instance and authentication settings, OIDC, storage, backups, and audit records; `gitadel-cli --help` lists them. The `api` command exposes other token-authorized endpoints. Browser-only account-security and restore flows keep their existing authentication requirements.

Backup downloads stream into a private temporary file and replace the requested output only after a complete transfer:

```bash
gitadel-cli admin backup providers list
gitadel-cli admin backup download "$PROVIDER_ID" "$BACKUP_KEY" --output backup.tar.zst
gitadel-cli admin backup progress "$OPERATION_ID"
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

## Git LFS storage

Git repositories, issue attachments, and release assets remain under the configured local storage roots. Administrators define tested filesystem and S3-compatible destinations under **Administration → Storage**. User-defined storage targets are also available as backup destinations; Gitadel does not create a default backup provider. Filesystem backups use a sibling `<storage-name>-backups` directory so an archive never contains itself; S3 backups use a `backups` child of the target prefix.

**Administration → Git LFS** shows object counts and logical space by repository and owner. Search repository names, filter by user or organization and byte range, and sort by name or space. The list starts with ten repositories; **Load more** fetches the next ten.

Changing the active target runs in the background without restarting Gitadel. Browsing, Git operations, and LFS downloads remain available. LFS uploads continue during the initial copy, then wait while Gitadel drains in-flight writes, copies the remaining objects, and commits the target change. Migration verifies object size and SHA-256 content. Source data is retained; a failed or interrupted migration leaves the source active and can be retried.

The server's offline `gitadel lfs` migration commands still require Gitadel to be stopped. They are separate from `gitadel-cli`, which manages the running instance:

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

Offline backups made with the `gitadel` server binary use a storage lock to guarantee one consistent snapshot. Stop the service before creating one:

```bash
mkdir -p backups
docker compose stop gitadel
docker compose run --rm -v "$PWD/backups:/backups" \
  gitadel backup create /backups/gitadel-backup.tar.zst
docker compose start gitadel
```

The archive contains the SQLite database (users, instance settings, credentials, and all other relational state), repositories and their attachments, LFS and release assets, the SSH host key, and the effective Gitadel configuration. Every file is covered by a SHA-256 manifest. Restore rejects changed, missing, or extra files and only writes database and storage data into empty configured paths:

```bash
gitadel backup restore /backups/gitadel-backup.tar.zst
```

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
