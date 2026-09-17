# Changelog

All notable changes to Gitadel are recorded here. This project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html) and the structure from [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- Added a built-in Docker/OCI registry on Gitadel's HTTP origin. Images belong to existing Git repositories and share their visibility, permissions, rename, deletion, and backup lifecycle. The registry supports API-token login, resumable uploads, layer mounts, image indexes, referrers, and content deletion.

## [0.7.1] - 2026-09-17

### Changed

- Docker Compose now starts a standalone Gitadel server with localhost-only published ports. The privileged Actions runner stack is opt-in through `compose.actions.yaml`.
- Container storage and listener defaults now use overridable environment variables, including when running offline backup commands.

### Fixed

- Copied the embedded changelog into the Docker build stage and declared the server's runtime library dependencies. The image builds only the server binary and excludes local configuration, secrets, and runtime data from its build context.

## [0.7.0] - 2026-09-17

### Added

- Added portable CLI login, status, and logout commands. Login validates API tokens before saving a private, server-bound credential file; token-file logins retain runtime references for secret rotation.

### Changed

- The NixOS CLI module now wraps the executable with server and token-file defaults instead of setting session-wide environment variables. SOPS secrets are read at runtime without placing token contents in the Nix store.
- Renamed the remote client executable to `gtd`. Cargo and Nix package/module names remain `gitadel-cli`; existing environment variables and saved credentials are unchanged.

## [0.6.2] - 2026-09-17

### Fixed

- Replaced recursive object traversal in Sley's integrity checker with an explicit work stack. Deep commit histories and nested trees or tags no longer overflow a worker thread's stack and abort the Gitadel server.
- Restored automatic NixOS service recovery after crashes with `Restart=on-failure`, retaining the two-second restart delay.

## [0.6.1] - 2026-09-17

### Added

- Added `GITADEL_TOKEN_FILE` and the NixOS client `tokenFile` option for runtime secret files, including SOPS-managed credentials. Explicit token flags and `GITADEL_TOKEN` retain precedence.
- Added a submodule detail panel with the upstream repository, pinned commit, tracking branch when configured, safe repository and commit links, and copyable checkout instructions. Metadata comes from `.gitmodules` at the selected revision without following includes or contacting external hosts.

### Fixed

- Display Git submodules as pinned repository references instead of requesting file contents. Submodule clicks, direct links, and browser history now show the commit ID and a link to `.gitmodules` when available.
- Removed an obsolete organization route parameter from the navigation rail, restoring frontend type checking.

## [0.6.0] - 2026-09-16

### Added

- Added per-repository Git LFS usage in administration, with user and organization ownership, name and owner search, byte-range filters, space and name sorting, and ten-entry pagination.
- Added separate Git and Git LFS storage sizes to repository sidebars for anyone with repository read access. File trees mark LFS pointers with a badge and show the referenced object size, including in nested directories and historical revisions.
- Added capacity and usage to each storage target in administration. Filesystem targets show a bar splitting what Gitadel stores from what else shares the volume, alongside the free space remaining; object stores publish no capacity, so those targets report usage alone. Cards load with the LFS total the database already tracks, and a scan walks the destination on request to account for backup archives and anything an earlier migration left behind.
- Added repository icons. Maintainers upload and crop one in repository settings, and repositories without a manual icon adopt a conventional `icon.png`, `logo.png`, or `favicon.png` committed to the default branch, including under `static/`, `public/`, `assets/`, and `.github/`. Detection reads only the repository itself and never contacts an external host. Repositories with no icon fall back to a monogram.
- Added a separate `gitadel-cli` binary for token-authenticated repository, organization, SSH key, and administrator management. It supports CI environment credentials, JSON input/output, streamed backup downloads, and reconnecting progress streams.
- Added independent Nix packages, apps, and NixOS modules for the server and client.

### Changed

- Removed the unused direct HTTP body utility dependency, consolidated SSH key handling on russh's key implementation, and replaced rust-embed with a generated asset lookup. Release builds still embed the frontend; debug builds read it from disk on each request.
- Replaced runtime Git, Git LFS, and external SSH processes with in-process Sley, HTTP, and SSH implementations. SeaORM remains the database layer.
- Preserved detached and unborn HEADs and custom ref namespaces during native imports, and corrected zero-object fetch responses, shallow negotiation, integrity validation, and reflog expiry/locking behavior.
- Split the repository sidebar's size readout into Git, LFS, and total cells, collapsing to a single figure when a repository stores nothing in LFS.
- Moved the branch selector into the file tree header, where the branch it scopes is, and reduced the navigation rail to repository views while a repository is open.
- Kept Gitadel online during administrator-triggered LFS storage migrations. Git operations and LFS downloads remain available; uploads continue during copying and wait only for final cutover. Failed migrations retain the source target.

- Unified the NixOS service lifecycle around `services.gitadel.enable`, `package`, and `autoStart`. The default package comes from the pinned flake; `autoStart = false` keeps Gitadel and enabled Gitadel-owned runner units manually startable without boot edges, while preserving the runner's dependency ordering and persisted state. The shared host Docker service remains under independent NixOS control.

## [0.5.1] - 2026-09-04

### Fixed

- Corrected the Nix package's frontend dependency hash and OpenSSL build input so the released package builds reproducibly.

## [0.5.0] - 2026-09-04

### Added

- Added `topic:name` filtering to the global repository search, backed by the topics already stored for each accessible repository.
- Added Gitadel Actions backed by separately deployed namespace-owned Forgejo Runner v13.0.0 pools. Push workflows get durable DAG scheduling, exact-label assignment, short-lived repository credentials, bounded paginated logs, cancellation and runner-loss handling, commit summaries, run/job views, Forgejo v4 artifact upload and download, and same-repository release publishing. Personal and organization owners manage one-time runner registrations under **Settings → Actions**; each runner can serve every repository in that namespace. The first release supports static push and tag workflows and intentionally defers pull requests, schedules, dispatch, reruns, matrices, reusable workflows, cache, OIDC, and user-managed Actions secrets or variables.
- Added provider-based instance backups. Administrators can add tested filesystem and S3-compatible destinations under **Settings → Backups**, choose the destination first when creating a snapshot, assign an independent schedule to each provider, and remove any backup setup. Gitadel does not create a default provider. Archives include the database, repositories, issue and release assets, LFS data, SSH host key, and effective configuration under an integrity-checked manifest. S3 backups use multipart upload, verified object sizes, and direct restore. Gitadel pauses normal services for maintenance, streams progress to the open page, creates a safety backup before restore by default, rolls back failed replacement attempts, and restarts automatically.
- Added token-authenticated Git Smart HTTP push and clone support on the web origin. Private clones require an API token with `read`; pushes require `write`.
- Added scheduled repository integrity checks. The daily pass runs strict Git object validation, reads every LFS object through SHA-256 verification, checks stored attachment sizes, and records its result in the administrator activity log. **Administration → Maintenance** controls whether checks run, stores the UTC cron schedule, and shows the latest result.
- Added pluggable Git LFS object storage with tested local-filesystem and S3-compatible targets. **Administration → Storage** defines and removes destinations; **Administration → Git LFS** reports object count and logical bytes used, selects the active target, and shows live byte progress through maintenance restart. User-defined storage targets are available for instance backups. Filesystem targets keep backup archives in a safe sibling directory; S3 targets use a dedicated child prefix. The offline CLI supports target creation, conformance testing, and restartable copy/verify/cutover migrations with durable batches, SHA-256 verification, atomic selection, and source retention. Bare repositories, issue attachments, and release assets remain independent and local unless configured otherwise.
- Added per-user light, dark, and system theme preferences. The documented shadcn mode switcher in the top bar saves the selection to the account, while new accounts default to the browser or operating-system color preference.
- Added repository mirrors. The **New** action clones HTTPS upstreams as read-only repositories, retains every ref including tags, and synchronizes manually, hourly, every six hours, or daily. Repository lists and headers mark mirrors, mirror settings report synchronization state and upstream errors, and a mirror can be converted into a writable repository without losing refs or imported content.
- Added namespace-owned mirror identities. Personal and organization namespaces can store named access tokens for a git server. Gitadel detects GitHub, GitLab, Gitea, or Forgejo when the identity is saved. Selecting an identity during mirror creation selects and locks its namespace; public mirrors need no identity. GitHub tokens also import the upstream website, topics, issues, labels, authors, and comments on every synchronization.
- Added namespace-first navigation. The sidebar now keeps Explore first, links personal repositories and favorites, lists every organization membership, and opens repository indexes at `/{namespace}`. Repository breadcrumbs link back to their namespace, and the global **New** action offers card-based repository and organization creation flows.
- Added a namespace integration catalog with named connection instances. Accounts and organizations can connect the same provider more than once, configure each instance in a modal, disable it without deleting credentials, or remove it. Repositories link to a specific instance rather than an ambiguous provider-wide credential.
- Added repository-level Dokploy setup. Gitadel now groups environments under their projects, lists Applications and Compose resources as cards, creates projects, environments, and resources, and safely links an existing target without replacing another repository's source. New Applications require only a name, branch, and optional deployment server. New targets start with automatic deployment disabled; the linked card opens the exact Dokploy resource page and keeps build, environment, domain, and runtime configuration in Dokploy. The three setup stages retain their selections and form values when revisited.
- Added integration credential tests. A namespace owner can verify the stored URL and API key against the live platform from **Account settings → Integrations** before wiring anything up.
- Added Dokploy source binding. Each connection now names one authorized Dokploy Gitea provider, can create that provider and its Gitadel OAuth application in one setup flow, reports source health, and blocks repository setup until the exact provider is ready. Gitadel cleans up providers it created but leaves pre-existing providers alone.
- Added webhook delivery history. Every webhook delivery (push, ping, and redelivery) is now recorded with its event, payload, response status, response body, duration, and timestamp, viewable per hook in repository settings with a redeliver action for retrying a past delivery.
- Added repository webhook delivery APIs: list deliveries (`GET /api/v1/repos/{namespace}/{name}/hooks/{id}/deliveries`), fetch one delivery, and redeliver it (`POST .../deliveries/{delivery_id}/attempts`). History is capped at the 50 most recent deliveries per webhook.
- Added integrations. An external service is connected once under **Account settings → Integrations** for an account or organization. The connection belongs to that namespace, so only its owner can read or change it and repository events reach only the owning namespace's service. Dokploy is the first supported provider. Stored API keys remain masked unless the namespace owner explicitly reveals one, and an unreachable service never fails the Git operation that emitted an event.
- Added per-repository integration opt-out. A repository inherits its owner's integrations by default and can be excluded individually under its **Settings → Integrations**, for forks, archives, and mirrors that should never deploy.
- Added push payload commits. Push webhook deliveries now carry the pushed commits with their added, modified, and removed paths (renames as a removal plus an addition) and a `head_commit`, so consumers filtering on changed paths — such as Dokploy watch paths — work instead of silently matching nothing.
- Added release and tag markers in commit history. A commit that carries a tag or a published release shows it as a chip and a marker on the timeline, with a separator naming the release, when it was published, and how many commits landed since the previous one. The commit page carries the same chips.
- Added provider-driven OpenID Connect authentication. Administrators manage password, passkey, and identity-provider access together under **Administration → Access**; every enabled provider appears directly on the sign-in page and can be disabled independently without a separate single sign-on switch.
- Added optional embedded HTTPS termination with PEM certificate and private-key configuration through TOML, environment variables, or CLI flags, allowing passkeys to work on trusted local and mesh-network origins without a separate reverse proxy.

### Changed

- Split the frontend's monolithic API client, repository page state, account settings state, and caches into domain-focused modules with shared validated transport and authorization-scoped loading. Split backup archives, integration transport, blob-store targets, and repository services into focused backend modules while preserving their public behavior.
- Unified repeated provider and connection management surfaces around reusable add, status, configure, enable, and remove cards across integrations, Actions runners, backups, storage, and OpenID Connect providers.
- Made the sign-in page remember and label the last successful password, passkey, or identity-provider method.
- Changed repository integrations from inherit-with-opt-out to explicit opt-in: each repository enables an integration under its own settings, and newly created repositories start disabled. Existing repositories keep their current behavior through a migration.
- Made repository integrations independently named instances. A repository can attach the same account or organization connection multiple times, rename each attachment, and give each one its own target and provider configuration. The add dialog now presents configured connections as cards and can create a new connection inline using the same reusable cards and credential editor as account settings.
- Made integrations provider-generic end to end: one registry entry owns provider metadata and its implementation, while repository events flow through a shared event/context contract. Provider-owned repository settings are validated before storage, and optional resource listing, linking, and manual actions remain capabilities rather than core assumptions.
- Made Dokploy repository-source setup use selectable provider cards and default new provider names to the administrator-configured instance name.
- Made username changes save on Enter without requiring the current password or a separate update button.
- Made general instance settings and favicon uploads save as soon as they change, removing the separate save step.
- Unified account and instance administration under one **Settings** route and moved its link to the bottom of the global sidebar. Administrators get Appearance, Access, Backups, and Activity sections alongside account settings. Repository and integration settings use the same section-first navigation, bordered panels, responsive layouts, and URL-addressable tabs.
- Split account security settings into focused Authentication, SSH keys, and API tokens pages, and moved mirror identities into each personal or organization namespace.
- Moved default repository visibility from instance administration to each account's settings. Repository creation now uses the creating account's preference when visibility is omitted.
- Made the repository file tree resizable on wide screens. Drag or keyboard-nudge the divider between the tree and the file preview to read deeply nested and long file names, and the chosen width is remembered per browser.
- Made webhook delivery history open on the five most recent deliveries, with a "Show more" button for the rest of the recorded history.
- Moved repository confirmations — pings, redeliveries, webhook and release changes, settings saves, issue updates — into toasts instead of a banner above the repository view.
- Made Dokploy setup a tested, staged flow. New connections must pass a credential check before they are saved, then continue to repository-source setup without closing the dialog. The connection also stores the Gitadel URL that Dokploy containers can reach and applies it to Gitadel-managed Gitea providers.
- Moved the repository branch selector into the repository toolbar. The file-tree header now shows the selected branch, commit ID, and relative commit age, links the exact timestamp on hover, and opens that branch's timestamped commit history.
- Updated the global search prompt glyph to use the active theme's foreground color instead of the accent color.

### Fixed

- Fixed repository settings failing with "The server returned an invalid response." The browser rejected identifiers that are not RFC 9562 version-tagged UUIDs, so a single legacy webhook row broke the whole settings view; identifiers are now validated by shape only.
- Fixed Gitadel-created Dokploy Gitea providers opening a doubled-slash OAuth URL that returned 404. Gitadel now removes the URL parser's trailing root slash before sending its public URL to Dokploy.
- Fixed **Deploy now** reporting a failure after Dokploy had already accepted an Application deployment. Self-hosted Dokploy returns an empty successful response, which Gitadel now accepts without attempting to decode as JSON.
- Fixed annotated tags pointing at their own tag object instead of the commit they name, which made the tags page and any tag-driven navigation resolve to an object that is not a commit.
- Fixed repository ZIP and TAR.GZ downloads returning an empty or invalid response when opened from the repository sidebar.

## [0.4.0] - 2026-08-22

### Added

- Added a shared application shell with breadcrumbs, a collapsible desktop rail, mobile navigation, an account menu, and repository creation from anywhere in the signed-in interface.
- Added a command palette, opened with Ctrl/Command+K or `/`, for searching repositories, reopening recent projects, and jumping to common destinations.
- Added profile-picture upload, cropping, zooming, keyboard repositioning, removal, and cache-busted avatar display. Gitadel stores the finished 512-pixel PNG and records changes in the audit log.
- Added repository storage size, including Git LFS objects, and commit totals to the repository metadata sidebar. Both calculations use bounded concurrency and server-side caches.
- Added a frontend type-check command and a flake-native development shell with the Rust toolchain pinned from `rust-toolchain.toml`.

### Changed

- Redesigned the repository browser as a full-width, three-column workspace with independent scrolling for the tree, file preview, and metadata sidebar on wide screens.
- Moved repository, account, and instance navigation into the shared rail, and simplified the Explore, Settings, Administration, and Changelog pages around the common shell.
- Kept repository clone controls, description, topics, counts, and language statistics visible across repository views. Branch selection now lives in the tree or metadata toolbar instead of a duplicate default-branch row.
- Made Explore search and favorites shareable through URL parameters, taught filtered views to load every available page, and made repository activity totals easier to scan.
- Optimized release binaries with full link-time optimization, single-unit code generation, abort-on-panic behavior, and symbol stripping.
- Made repository pages render their critical content before supplementary size and commit-count scans finish, then refresh those metrics in the background.
- Reworked repository discovery to authorize repositories in bulk, parallelized overview analysis, bounded expensive Git work, and cached repository overviews, language statistics, commit totals, and storage measurements.
- Added intent-based repository preloading, idle command-palette warming, cached repository indexes, and deduplicated application-state initialization.
- Kept the complete Material Icon Theme while moving its manifest and 1,250 SVGs out of the JavaScript graph; versioned icons now load on demand and use immutable browser caching.
- Added Brotli and gzip response compression plus immutable cache headers for versioned frontend assets.
- Lazy-loaded syntax highlighting, bounded rich diff rendering, and added a fast unified fallback for large patches.
- Replaced the devenv setup with direnv and `nix develop`, and made release builds install frozen frontend dependencies and require the Cargo lockfile.

### Fixed

- Added a selection-based clipboard fallback for clone URLs on plain HTTP or in browsers that deny the Clipboard API, with bottom-right Sonner notifications for success and failure.
- Kept language statistics alive while switching repository tabs, loaded them when opening a non-code view directly, and cancelled obsolete repository requests after route changes.
- Pointed Explore's infinite-scroll observer at the application shell's scroll container.
- Kept long source lines and rendered README content inside their own preview scroller instead of stretching the repository page.
- Invalidated cached repository sizes after pushes and LFS uploads.
- Included the Pierre diff renderer dependency used by commit patch views.
- Prevented concurrent repository-size invalidation and background measurement from retaining stale results.
- Prevented oversized source files and commit patches from monopolizing the browser main thread.

### Removed

- Removed duplicated page headers, the old repository header, and route-local navigation and repository-creation controls now owned by the application shell.
- Removed the devenv configuration files.
- Removed the Rust test modules, Cargo test harness, Nix package checks, and NixOS virtual-machine test.

## [0.3.0] - 2026-08-22

### Added

- Added a changelog page that renders the release notes embedded in the running binary, so an instance always documents the version it is serving.

### Changed

- Committed repository topics as soon as Enter, space, or comma is pressed and removed the save and cancel buttons, so editing a topic list no longer needs a separate confirmation step.
- Outlined topic badges so they separate from the sidebar background.
- Turned the default branch setting into a list of the repository's existing branches instead of a free-text field.
- Split repository settings into General, Location, Webhooks, and Danger zone tabs.
- Moved editing the repository description out of the settings page, leaving the in-place editor on the repository itself as the only place it is changed.

### Fixed

- Rendered sanitized HTML, images, badges, and repository-relative links in Markdown files and repository READMEs.

## [0.2.0] - 2026-08-22

### Added

- Added a repository settings view covering renames, transfers between owned namespaces, visibility, and the default branch, leaving an alias behind so old clone URLs keep working.
- Added a repository lifecycle of archiving, soft deletion with a recovery period, restoration, and permanent purging.
- Added repository topics stored in their own table, editable from the sidebar, with suggestions drawn from every repository the viewer can already see.
- Added in-place editing of the repository description from the sidebar.
- Added repository push webhooks with GitHub-style CRUD and ping APIs, signed deliveries, delivery status, and repository settings controls.
- Added account settings for changing usernames and passwords, including personal repository namespace updates and revocation of other browser sessions after password changes.
- Added modal SSH-key and API-token creation, one-time token copying, destructive-action confirmations, and Sonner notifications across account settings.
- Added OAuth application management to settings, covering registration, secret rotation, and revocation.
- Added administrator-supplied light and dark favicons stored in the database, with the bundled marks served as fallbacks.
- Added a paginated repository overview so large instances no longer load every repository at once.
- Added NixOS module options for the database URL, session and invitation lifetimes, secrets through an `environmentFile`, and declarative bootstrapping of the first administrator.
- Added a `nix develop` shell, a flake formatter, and a NixOS virtual-machine test that boots the service, bootstraps an administrator, and restarts it.

### Changed

- Bootstrapped the repository page from a single preloaded request set and cached navigation payloads, so moving between pages no longer refetches unchanged data.

### Fixed

- Stopped the repository toolbar from shifting when switching between the code and settings views.
- Enforced repository-specific OAuth scopes for repository discovery, Git HTTP cloning, and LFS access.
- Preserved the OAuth applications settings destination when authentication is required.
- Hardened OAuth consent and token responses against framing and credential caching.
- Repaired `nix build` by refreshing the stale frontend dependency hash and giving the sandbox a CA bundle for the webhook tests.
- Scoped the frontend dependency derivation to the manifest and lockfile so editing frontend sources no longer reinstalls packages.
- Defaulted `services.gitadel.package` to the flake's own build and stopped the NixOS module from setting `nixpkgs.overlays`, which conflicted with configurations that set `nixpkgs.pkgs`.
- Granted `CAP_NET_BIND_SERVICE` when Gitadel listens on a privileged HTTP or SSH port, and delegated state directory creation to systemd.

## [0.1.0] - 2026-08-22

Initial release.

### Added

- Public and private repositories under user and organization namespaces, with favorites, search, filtering, and daily commit activity.
- SSH push and clone, repository creation on first push, and Smart HTTP fetch for public repositories.
- Git LFS object transfer, file locking, lock verification, and unlocking.
- Repository browsing for branches, tags, trees, files, raw content, commit history, commit details, and diffs.
- Rendered Markdown, syntax-highlighted source and fenced code blocks, Material file icons, and per-language source statistics.
- Password and passkey authentication, server-side sessions, invitations, SSH keys, and scoped API tokens.
- OAuth 2.0 authorization-code applications with consent, scoped access tokens, and token authentication.
- Gitea-compatible repository and branch API endpoints for external clients.
- Organization membership, repository collaborators, visibility controls, and per-repository access grants.
- Administrator instance settings and an audit log for security-sensitive actions.
- CLI commands for repository creation and integrity-checked offline backup and restore.
- Docker Compose and NixOS deployment, a portable SQLite-backed data directory, and an embedded SvelteKit frontend.

[Unreleased]: https://github.com/Fractal-Tess/gitadel/compare/v0.7.1...HEAD
[0.7.1]: https://github.com/Fractal-Tess/gitadel/compare/v0.7.0...v0.7.1
[0.7.0]: https://github.com/Fractal-Tess/gitadel/compare/v0.6.2...v0.7.0
[0.6.2]: https://github.com/Fractal-Tess/gitadel/compare/v0.6.1...v0.6.2
[0.6.1]: https://github.com/Fractal-Tess/gitadel/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/Fractal-Tess/gitadel/compare/v0.5.1...v0.6.0
[0.5.1]: https://github.com/Fractal-Tess/gitadel/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/Fractal-Tess/gitadel/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/Fractal-Tess/gitadel/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/Fractal-Tess/gitadel/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/Fractal-Tess/gitadel/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/Fractal-Tess/gitadel/releases/tag/v0.1.0
