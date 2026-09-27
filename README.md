<p align="center">
  <img src="assets/gitadel-favicon.svg" alt="Gitadel" width="160" />
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

- **Store** public and private repositories under user or organization namespaces, with access controls, Git LFS, file locks, and a built-in container registry.
- **Browse** branches, tags, history, diffs, issues, rendered Markdown, image previews, syntax-highlighted source, topics, and language statistics.
- **Mirror** public or token-authenticated repositories over HTTPS, keep every ref synchronized, import GitHub topics and issues, and convert a mirror into a writable repository without losing its contents.

Gitadel deliberately leaves out pull requests and social feeds. The browser can create files and tracked directories, but does not edit existing files.

Repository operations run in-process through [Sley](https://github.com/Fractal-Tess/sley). The server does not require installed Git, Git LFS, or SSH executables.

## Quick start

```bash
curl -fsSLO https://raw.githubusercontent.com/Fractal-Tess/gitadel/main/compose.yaml
docker compose up -d
docker compose logs gitadel
```

Open the one-time setup link from the log to create the administrator. HTTP listens on `3000`, SSH on `2222`, and everything persists in the `gitadel-data` volume. Use [`compose.actions.yaml`](compose.actions.yaml) instead for a built-in Actions runner. See [INSTALL.md](INSTALL.md) for settings, hosting platforms such as Coolify, NixOS, and backups.

Add your SSH key under **Account settings**, create a repository, and push:

```bash
git remote add archive ssh://git@localhost:2222/archivist/old-project.git
git push archive main
```

Press **Ctrl+K** to search repositories or find commands for creation, imports, and settings.

## Command-line client

`gtd` manages a running instance with an API token. It is separate from the `gitadel` server and uses the same permissions as the web API.

Download it from the [latest release](https://github.com/Fractal-Tess/gitadel/releases/latest) for Linux, macOS, or Windows, or install it with Nix:

```bash
nix profile install github:Fractal-Tess/gitadel#gitadel-cli
gtd auth login
gtd repo list
gtd admin instance get
```

Create a token under **Account settings → Access**, then enter the server URL and token at the login prompts. The CLI remembers them for later commands. NixOS users can instead point the client at a runtime SOPS secret. See [client setup, credential storage, and CI usage](INSTALL.md#command-line-client).

## Container images

Push Docker and OCI images to `git.example.com/owner/repository:tag` on the same origin as Gitadel. The Git repository must exist first; its visibility and permissions apply to the images. Browse images, tags, sizes, and pull commands from the repository's **Container registry** tab. See [registry setup, authentication, and limits](INSTALL.md#container-registry).

## Dokploy

Gitadel implements the Gitea OAuth and repository APIs used by Dokploy. Dokploy can discover accessible repositories and branches, clone them with repository-scoped OAuth tokens, and deploy on push. Add one or more named Dokploy connections to an account or organization, then choose which connection each repository should use—without adding a webhook per repository.


## Project documentation

- [Installation and operations](INSTALL.md)
- [Development and contributing](CONTRIBUTING.md)
- [Release history](CHANGELOG.md)

## License

MIT. See [LICENSE](LICENSE).
