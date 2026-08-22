{
  description = "Gitadel archival Git server";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, rust-overlay }:
    let
      inherit (nixpkgs) lib;
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = lib.genAttrs systems;
      # Read from the manifest so the flake cannot drift from the crate version.
      version = (lib.importTOML ./Cargo.toml).package.version;

      buildDeps = pkgs: [
        pkgs.cmake
        pkgs.git
        pkgs.perl
        pkgs.pkg-config
      ];

      packageFor = pkgs:
        let
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
            outputHash = "sha256-g2EWpZND49WmKcCFrvH3XmbgQoH/GJoHg0A6G0eEqbY=";
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
        in
        pkgs.rustPlatform.buildRustPackage {
          pname = "gitadel";
          inherit version;
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          doCheck = false;
          nativeBuildInputs = buildDeps pkgs ++ [ pkgs.makeWrapper ];
          preBuild = ''
            rm -rf frontend/build
            cp -R ${frontend} frontend/build
          '';
          postInstall = ''
            wrapProgram $out/bin/gitadel \
              --prefix PATH : ${lib.makeBinPath [ pkgs.git ]}
          '';
          passthru = { inherit frontend nodeModules; };
          meta = {
            description = "A minimal self-hosted Git server for archival repositories";
            homepage = "https://github.com/Fractal-Tess/gitadel";
            license = lib.licenses.mit;
            mainProgram = "gitadel";
            platforms = lib.platforms.linux;
          };
        };
    in
    {
      packages = forAllSystems (system:
        let gitadel = packageFor nixpkgs.legacyPackages.${system}; in
        {
          inherit gitadel;
          default = gitadel;
        });

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = lib.getExe self.packages.${system}.gitadel;
        };
      });

      devShells = forAllSystems (system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
          devCommand = name: text: pkgs.writeShellApplication {
            inherit name text;
            runtimeInputs = [ pkgs.bun pkgs.git pkgs.nix rustToolchain ];
          };
        in
        {
          default = pkgs.mkShell {
            packages = buildDeps pkgs ++ [
              pkgs.bun
              pkgs.curl
              pkgs.jq
              pkgs.openssh
              pkgs.rust-analyzer
              rustToolchain
              (devCommand "backend" ''
                cargo run -- "$@"
              '')
              (devCommand "frontend" ''
                bun run --cwd frontend dev "$@"
              '')
              (devCommand "frontend-install" ''
                bun install --cwd frontend --frozen-lockfile "$@"
              '')
              (devCommand "frontend-build" ''
                bun run --cwd frontend build "$@"
              '')
              (devCommand "release-build" ''
                exec ./scripts/build-release.sh "$@"
              '')
              (devCommand "frontend-hash" ''
                exec ./scripts/update-frontend-hash.sh "$@"
              '')
            ];
          };
        });

      formatter = forAllSystems (system: nixpkgs.legacyPackages.${system}.nixpkgs-fmt);

      overlays.default = final: _prev: {
        gitadel = self.packages.${final.stdenv.hostPlatform.system}.gitadel;
      };

      # Deliberately does not set `nixpkgs.overlays`: that conflicts with
      # `nixpkgs.pkgs`, which flake-parts and shared-pkgs setups commonly set.
      nixosModules.gitadel = { pkgs, ... }: {
        imports = [ ./nix/module.nix ];
        services.gitadel.package =
          lib.mkDefault self.packages.${pkgs.stdenv.hostPlatform.system}.gitadel;
      };

      nixosModules.default = self.nixosModules.gitadel;
    };
}
