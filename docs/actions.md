# Gitadel Actions

Gitadel Actions is a repository-scoped CI control plane. Gitadel discovers workflows after successful pushes, schedules durable runs and jobs, authorizes a runner, stores logs, and reports commit status. The separately deployed Forgejo Runner executes repository code. Gitadel itself never executes workflow steps.

This is a pinned Forgejo Runner integration, not a claim of generic GitHub Actions compatibility.

## Trust and isolation

Workflow discovery is active for every repository. A pushed workflow can read that repository with the job's short-lived credential, publish releases and release assets to that same repository, and exfiltrate those values through transformed logs or the network. Registering a runner therefore enables code execution for every repository in its personal or organization namespace.

Run Forgejo Runner on a dedicated disposable host or VM. Configure a container backend, non-privileged containers, no host or Docker socket mounts, explicit CPU/memory/process limits, a read-only host filesystem where practical, and restricted egress. The runner protocol does not let Gitadel attest any of those controls. Do not register a runner configured for host execution.

A namespace runner can serve every repository owned by that user or organization. Each runner executes one job at a time; register more runners when the namespace needs concurrency. Gitadel issues each job a short-lived token limited to repository reads, optional LFS reads, and release/release-asset publication for that same repository. It cannot push or write LFS, and terminal completion or cancellation revokes it.

## Required runner

The compatibility pin is exactly Forgejo Runner **v13.0.0** and `actions-proto` **v0.7.0**. Other runner versions are rejected.

Official Linux amd64 binary:

```text
https://code.forgejo.org/forgejo/runner/releases/download/v13.0.0/forgejo-runner-13.0.0-linux-amd64
sha256 cfcfe65e9ed5c9a4b344acfdbc3c32210b274c005a5888f46967c4d988177707
```

The official OCI package is `code.forgejo.org/forgejo/runner:13.0.0`. Pin the resolved image digest in deployment configuration rather than tracking `13`, `13.0`, or another mutable tag.

## Register a runner

1. Open **Settings → Actions**.
2. Choose your personal namespace or an organization you own.
3. Enter a runner name and comma-separated label names. Labels are exact, case-sensitive names such as `docker`.
4. Select **Create registration token**. The token is displayed once and expires after 10 minutes.
5. Put the execution mapping for those label names in the runner configuration. The server approves names; the runner owns their container mapping.
6. Run the registration command shown by Gitadel, then start `forgejo-runner daemon` with the same configuration.

Minimal isolated mapping:

```yaml
runner:
  capacity: 1
  labels:
    docker:
      backend: docker
      backend-options:
        image: docker.io/library/node@sha256:<immutable-image-digest>

cache:
  enabled: false

container:
  privileged: false
  valid_volumes: []
  docker_host: "-"
```

The generated command has this form:

```sh
forgejo-runner register --no-interactive \
  --instance https://git.example.com \
  --token gta_reg_REDACTED \
  --name repository-runner \
  --labels docker
forgejo-runner --config /etc/forgejo-runner/config.yml daemon
```

Protect the registration and runner-token files as credentials. Removing a runner in Gitadel revokes its access to the namespace. Online/offline state comes from authenticated runner requests; the browser does not infer it.

## Workflows

Gitadel reads workflows from the pushed commit, never from the working tree or default branch. It uses the first existing directory in this order:

1. `.forgejo/workflows`
2. `.gitea/workflows`
3. `.github/workflows`

Lower-precedence directories are ignored when a higher-precedence directory exists. YAML files may use `.yml` or `.yaml`.

Supported scope:

- `push`, including `branches`, `branches-ignore`, `tags`, `tags-ignore`, `paths`, and `paths-ignore`
- static job DAGs with `needs`
- one or more static `runs-on` labels
- shell steps and runner-evaluated step/job conditions
- step outputs and downstream `needs` results/outputs
- same-commit local actions
- JavaScript and Docker actions pinned to a full 40-character commit from the configured action origin
- job containers and services pinned by `@sha256:<64 hex characters>`
- Forgejo v4 artifact upload and download actions
- tagged releases and streaming release-asset uploads through the pinned Forgejo release action

Example:

```yaml
name: Verify
on:
  push:
    branches: [main]

jobs:
  test:
    runs-on: docker
    outputs:
      result: ${{ steps.result.outputs.value }}
    steps:
      - run: cargo test --locked
      - id: result
        run: echo "value=passed" >> "$GITHUB_OUTPUT"

  report:
    needs: test
    runs-on: docker
    steps:
      - run: test "${{ needs.test.outputs.result }}" = passed
```

Artifact actions use the configured default origin. Pin the Forgejo actions to immutable commits:

```yaml
- uses: forgejo/upload-artifact@16871d9e8cfcf27ff31822cac382bbb5450f1e1e
  with:
    name: production-build
    path: build

- uses: forgejo/download-artifact@d8d0a99033603453ad2255e58720b460a0555e1e
  with:
    name: production-build
    path: downloaded
```

Publish a release from a tag push with the same short-lived repository token. This pinned action revision has no cache-action dependency:

```yaml
on:
  push:
    tags: ["v*"]

jobs:
  release:
    runs-on: docker
    steps:
      - run: |
          mkdir -p dist/release
          cp build/my-app dist/release/
      - uses: actions/forgejo-release@5edac7de8780f9fa306a7a7e26e88da30f70b8db
        with:
          direction: upload
          release-dir: dist/release
          token: ${{ secrets.FORGEJO_TOKEN }}
          release-notes: |
            Automated release for ${{ github.ref_name }}.
```

Malformed or unsupported workflows create a visible failed run with no jobs. They are never silently reduced to a supported subset.

Not supported: pull-request triggers, manual dispatch, schedules, reruns, matrix expansion, reusable workflows or job-level `uses`, concurrency groups, environments/approvals, deployments, cache, OIDC, or user-managed Actions secrets/variables.

## Configuration

Actions settings live under `[actions]` in `gitadel.toml` and support the equivalent `GITADEL_ACTIONS__...` environment keys.

```toml
[actions]
allowed_action_origins = ["https://code.forgejo.org"]
default_actions_origin = "https://code.forgejo.org"
runner_loss_seconds = 300
fetch_timeout_seconds = 20
max_log_request_bytes = 1048576
max_log_row_bytes = 65536
max_log_response_bytes = 262144
max_job_log_bytes = 16777216
retention_days = 90
max_artifact_bytes = 2147483648
max_artifact_upload_request_bytes = 16777216
max_artifact_block_list_bytes = 1048576
max_artifact_blocks = 50000
max_artifact_name_bytes = 255
artifact_grant_lifetime_seconds = 3600
lfs_read = true
```

`allowed_action_origins` is empty by default, so remote actions are disabled until an operator opts in. `default_actions_origin` must also appear in that list before remote actions are accepted. Local actions and pinned container actions remain subject to workflow validation.

A runner that stops reporting past `runner_loss_seconds` fails its current job with `runner_lost`; Gitadel does not retry it automatically. The default is five minutes. Job logs accept at most 1 MiB per request and 64 KiB per row, return at most 256 KiB per page, and retain at most 16 MiB per job with a visible truncation marker. Workflow artifacts default to a 2 GiB archive limit, 16 MiB upload requests, 50,000 blocks, 255-byte names, and one-hour signed grants; the workflow's requested retention is capped by `retention_days`.

Workflow limits are 64 files in the selected directory, 1 MiB per file, 128 jobs per workflow, 256 dependency edges, and 256 steps per job.

## Runner deployment

Gitadel supports three runner scopes:

- **Personal namespace**: every repository owned by one user.
- **Organization namespace**: every repository owned by one organization.
- **System**: every repository on the instance. Only administrators can manage this scope.

Repository jobs select all eligible runners from their namespace plus the system pool. A namespace runner never receives another namespace's jobs. A system runner receives the oldest compatible queued job across the instance. `runs-on` labels must match the runner's labels exactly.

Manage personal and organization runners from the namespace **Runners** tab. Manage system runners under **Administration → Runners**. Each registration token is single-use, expires after ten minutes, and is locked to the displayed runner name, labels, and scope.

### Docker Compose

The root `compose.yaml` includes Gitadel, a Forgejo Runner, and an isolated Docker-in-Docker daemon. The runner registers itself from a one-time token generated by Gitadel; no token copying or repository-by-repository setup is required.

```bash
GITADEL_PUBLIC_URL=http://gitadel.example.test:3000 docker compose up -d --build
```

Set `GITADEL_PUBLIC_URL` to the address clients use. The Compose runner talks to Gitadel over the private Compose network. Workflow containers run through the dedicated Docker daemon, not the host Docker socket. Its default memory limit is 4 GiB; override it with `RUNNER_MEMORY_LIMIT`.

Persistent volumes:

- `gitadel-data`: repositories, database, SSH key, artifacts, and the bootstrap registration token.
- `runner-data`: Forgejo Runner registration and state.
- `runner-docker`: isolated workflow-container storage.

Check the services and runner:

```bash
docker compose ps
docker compose logs runner
```

The runner is ready when its log reports `declared successfully`. Gitadel then shows it as **Online** under **Administration → Runners**.

### NixOS

Enable the bundled system runner in the Gitadel module:

```nix
services.gitadel = {
  enable = true;
  autoStart = true;
  publicUrl = "https://gitadel.example.test";

  runner = {
    enable = true;
    name = "gitadel-system";
    labels = [ "docker" ];
    memoryBytes = 4 * 1024 * 1024 * 1024;
  };
};
```

NixOS creates `gitadel-runner` and `gitadel-runner-docker`. Both Gitadel-owned units follow `services.gitadel.autoStart`, which defaults to `true`; with `autoStart = false`, they remain available for manual startup and the runner's `requires` edges still start Gitadel and its isolated Docker daemon in the right order. The shared host `docker.service` is managed independently by the NixOS Docker module. The former runs the pinned Forgejo Runner image; the latter is a privileged but isolated Docker daemon capped by `runner.memoryBytes`. Gitadel writes a mode-`0600` one-time registration token, the runner consumes and deletes it, and the persisted `.runner` registration is reused on restart.

Inspect the services with:

```bash
systemctl status gitadel gitadel-runner gitadel-runner-docker
journalctl -u gitadel-runner -f
```

Keep the runner on a trusted machine. Workflow code can control its job container and may exfiltrate repository-scoped credentials available to that job. Do not expose the isolated daemon's TCP port or mount the host Docker socket into the runner.
