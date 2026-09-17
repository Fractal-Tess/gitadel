{
  description = "Gitadel archival Git server";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
    }:
    let
      inherit (nixpkgs) lib;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = lib.genAttrs systems;
      # Read from the manifest so the flake cannot drift from the crate version.
      version = (lib.importTOML ./Cargo.toml).workspace.package.version;

      buildDeps = pkgs: [
        pkgs.cmake
        pkgs.perl
        pkgs.pkg-config
      ];

      packageFor =
        pkgs:
        let
          rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
          rustPlatform = pkgs.makeRustPlatform {
            cargo = rustToolchain;
            rustc = rustToolchain;
          };
          # The CLI source intentionally excludes the server and frontend trees.
          clientSource = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./rust-toolchain.toml
              ./cli
            ];
          };
          commonRustAttrs = {
            inherit version;
            cargoLock.lockFile = ./Cargo.lock;
            cargoLock.outputHashes."sley-0.10.0" = "sha256-ZSETbJhpqXVb+tlmySXAKPDcXo8suxMVhjsyNha2dJc=";
            doCheck = false;
          };
          # Only the manifest and lockfile, so editing frontend sources does not
          # invalidate the fixed-output derivation. Refresh `outputHash` with
          # ./scripts/update-frontend-hash.sh whenever bun.lock changes.
          dependencySource = lib.fileset.toSource {
            root = ./frontend;
            fileset = lib.fileset.unions [
              ./frontend/package.json
              ./frontend/bun.lock
            ];
          };
          nodeModules = pkgs.stdenv.mkDerivation {
            pname = "gitadel-frontend-node-modules";
            inherit version;
            src = dependencySource;
            nativeBuildInputs = [ pkgs.bun ];
            dontConfigure = true;
            dontFixup = true;
            buildPhase = ''
              runHook preBuild
              export HOME=$TMPDIR
              export BUN_INSTALL_CACHE_DIR=$TMPDIR/bun-cache
              bun install --frozen-lockfile --ignore-scripts --no-progress
              runHook postBuild
            '';
            installPhase = ''
              runHook preInstall
              mkdir -p $out
              cp -R node_modules $out/
              runHook postInstall
            '';
            outputHash = "sha256-HCNSaX62NMvNtBxWF6CAg1UunohZBQwEg/MCrka8M9s=";
            outputHashAlgo = "sha256";
            outputHashMode = "recursive";
          };
          frontend = pkgs.stdenv.mkDerivation {
            pname = "gitadel-frontend";
            inherit version;
            src = ./frontend;
            nativeBuildInputs = [ pkgs.bun ];
            dontConfigure = true;
            buildPhase = ''
              runHook preBuild
              cp -R ${nodeModules}/node_modules .
              chmod -R u+w node_modules
              patchShebangs node_modules
              bun node_modules/vite/bin/vite.js build
              runHook postBuild
            '';
            installPhase = ''
              runHook preInstall
              cp -R build $out
              runHook postInstall
            '';
          };
          server = rustPlatform.buildRustPackage (
            commonRustAttrs
            // {
              pname = "gitadel";
              src = ./.;
              nativeBuildInputs = buildDeps pkgs;
              buildInputs = [ pkgs.openssl ];
              cargoBuildFlags = [
                "--package"
                "gitadel"
                "--bin"
                "gitadel"
              ];
              preBuild = ''
                rm -rf frontend/build
                cp -R ${frontend} frontend/build
              '';
              passthru = { inherit frontend nodeModules; };
              meta = {
                description = "A minimal self-hosted Git server for archival repositories";
                homepage = "https://github.com/Fractal-Tess/gitadel";
                license = lib.licenses.mit;
                mainProgram = "gitadel";
                platforms = lib.platforms.linux;
              };
            }
          );
          client = rustPlatform.buildRustPackage (
            commonRustAttrs
            // {
              pname = "gitadel-cli";
              src = clientSource;
              cargoBuildFlags = [
                "--package"
                "gitadel-cli"
                "--bin"
                "gtd"
              ];
              meta = {
                description = "Token-authenticated Gitadel command-line client";
                homepage = "https://github.com/Fractal-Tess/gitadel";
                license = lib.licenses.mit;
                mainProgram = "gtd";
                platforms = lib.platforms.linux;
              };
            }
          );
        in
        {
          gitadel = server;
          gitadel-cli = client;
        };
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          packagesFor = packageFor pkgs;
        in
        {
          gitadel = packagesFor.gitadel;
          gitadel-cli = packagesFor.gitadel-cli;
          default = packagesFor.gitadel;
        }
      );

      apps = forAllSystems (system: {
        gitadel = {
          type = "app";
          program = lib.getExe self.packages.${system}.gitadel;
          meta.description = "Gitadel Git archive server";
        };
        gitadel-cli = {
          type = "app";
          program = lib.getExe self.packages.${system}.gitadel-cli;
          meta.description = "Token-authenticated Gitadel command-line client";
        };
        default = {
          type = "app";
          program = lib.getExe self.packages.${system}.gitadel;
          meta.description = "Gitadel Git archive server";
        };
      });

      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        in
        {
          default = pkgs.mkShell {
            packages = buildDeps pkgs ++ [
              pkgs.git
              pkgs.git-lfs
              pkgs.bun
              pkgs.curl
              pkgs.jq
              pkgs.mkcert
              pkgs.nssTools
              pkgs.just
              pkgs.openssh
              pkgs.process-compose
              pkgs.rust-analyzer
              pkgs.watchexec
              rustToolchain
            ];
          };
        }
      );

      formatter = forAllSystems (system: nixpkgs.legacyPackages.${system}.nixpkgs-fmt);
      # Deliberately does not set `nixpkgs.overlays`: that conflicts with
      # `nixpkgs.pkgs`, which flake-parts and shared-pkgs setups commonly set.
      nixosModules.gitadel = import ./nix/module.nix { inherit self; };
      nixosModules.gitadel-cli = import ./nix/client-module.nix { inherit self; };

      nixosModules.default = self.nixosModules.gitadel;

    };
}
