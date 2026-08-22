# Changelog

All notable changes to Gitadel are recorded here. This project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html) and the structure from [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

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

[Unreleased]: https://github.com/Fractal-Tess/gitadel/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/Fractal-Tess/gitadel/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/Fractal-Tess/gitadel/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/Fractal-Tess/gitadel/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/Fractal-Tess/gitadel/releases/tag/v0.1.0
