{ pkgs, ... }:

{
  packages = with pkgs; [
    bun
    git
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
  };
}
