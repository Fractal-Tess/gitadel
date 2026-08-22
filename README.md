<p align="center">
  <img src="assets/gitadel-logo.png" alt="Gitadel" width="160" />
</p>

<p align="center">
  <a href="https://github.com/Fractal-Tess/gitadel/tags"><img src="https://img.shields.io/github/v/tag/Fractal-Tess/gitadel?sort=semver&color=f97316" alt="Latest version" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-2b2b2b.svg" alt="MIT license" /></a>
</p>

<h1 align="center">Gitadel</h1>

<p align="center">
  A small self-hosted Git server for projects you want to keep.
</p>

Gitadel keeps the useful parts of a forge without becoming another collaboration platform. It is for individuals and small teams that want to push repositories over SSH, browse them on the web, and keep the entire instance in one portable data directory.

- **Store** public and private repositories under user or organization namespaces, with Git LFS and file locks.
- **Browse** branches, tags, history, diffs, rendered Markdown, syntax-highlighted source, topics, and language statistics.
- **Control access** with passwords, passkeys, invitations, SSH keys, scoped API tokens, OAuth applications, and repository grants.

Gitadel deliberately leaves out pull requests, issues, social features, and in-browser editing.

## Quick start

```bash
git clone https://github.com/Fractal-Tess/gitadel.git
cd gitadel
docker compose up --build
```

Open [http://localhost:3000/register](http://localhost:3000/register) to create the first administrator. HTTP listens on `3000`, SSH listens on `2222`, and persistent state is stored in the `gitadel-data` volume.

Use **New repository** in the web UI, add your SSH key under **Account settings**, and push:

```bash
git remote add archive ssh://git@localhost:2222/archivist/old-project.git
git push archive main
```

See [INSTALL.md](INSTALL.md) for Docker, NixOS, configuration, reverse-proxy, and backup instructions.

## Dokploy

Gitadel implements the Gitea OAuth and repository APIs used by Dokploy. Dokploy can discover accessible repositories and branches, clone them with repository-scoped OAuth tokens, and receive signed push webhooks for automatic deployments.

See [the Dokploy integration guide](docs/dokploy.md) for setup.

## Project documentation

- [Installation and operations](INSTALL.md)
- [Development and contributing](CONTRIBUTING.md)
- [Release history](CHANGELOG.md)

## License

MIT. See [LICENSE](LICENSE).
