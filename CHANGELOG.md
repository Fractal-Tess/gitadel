# Changelog

All notable changes to Gitadel are recorded here. This project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html) and the structure from [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

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

[Unreleased]: https://github.com/Fractal-Tess/gitadel/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Fractal-Tess/gitadel/releases/tag/v0.1.0
