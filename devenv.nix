{ pkgs, ... }:

{
  packages = with pkgs; [
    curl
    bun
    git
    jq
    python3
    openssh
  ];

  languages.rust = {
    enable = true;
    toolchainFile = ./rust-toolchain.toml;
  };

  scripts = {
    backend.exec = "cargo run -- $@";
    frontend.exec = "bun run --cwd frontend dev";
    frontend-install.exec = "bun install --cwd frontend --frozen-lockfile";
    frontend-build.exec = "bun run --cwd frontend build";
    release-build.exec = "./scripts/build-release.sh";
    frontend-hash.exec = "./scripts/update-frontend-hash.sh";
  };
}
