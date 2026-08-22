# Changelog

All notable changes to Gitadel are recorded here. This project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html) and the structure from [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

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

[Unreleased]: https://github.com/Fractal-Tess/gitadel/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/Fractal-Tess/gitadel/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/Fractal-Tess/gitadel/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/Fractal-Tess/gitadel/releases/tag/v0.1.0
