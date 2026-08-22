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
            publicUrl = "https://git.example.com";
            openFirewall = true;
          };
        }
      ];
    };
  };
}
```

The module runs Gitadel as a hardened systemd service and stores persistent state in `/var/lib/gitadel`. `services.gitadel.package` defaults to the flake build and only needs to be set to override it.

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
services.gitadel.environmentFile = "/run/secrets/gitadel.env";
```

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

The flake exports `overlays.default` as another way to provide `pkgs.gitadel`. The module can also be imported directly from `nix/module.nix`; it does not set `nixpkgs.overlays`, so it composes with configurations that provide their own `nixpkgs.pkgs`.

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

## Backups

Backups are offline so the storage lock can guarantee one consistent snapshot. Stop the service before creating one:

```bash
mkdir -p backups
docker compose stop gitadel
docker compose run --rm -v "$PWD/backups:/backups" \
  gitadel backup create /backups/gitadel-backup.tar.zst
docker compose start gitadel
```

Each archive includes a SHA-256 manifest. Restore rejects changed, missing, or extra files and only writes into empty configured storage paths. Run `gitadel backup restore --help` for the restore command.
