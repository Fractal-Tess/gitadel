{
  description = "Gitadel archival Git server";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs = { self, nixpkgs }:
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
          nativeBuildInputs = buildDeps pkgs ++ [ pkgs.makeWrapper ];
          preBuild = ''
            rm -rf frontend/build
            cp -R ${frontend} frontend/build
          '';
          # The webhook tests build a rustls client, which needs a CA bundle that
          # the sandbox does not otherwise provide.
          nativeCheckInputs = [ pkgs.cacert ];
          preCheck = ''
            export SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt
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
        let pkgs = nixpkgs.legacyPackages.${system}; in
        {
          # `devenv.nix` remains the primary shell and pins the exact Rust
          # toolchain; this one exists so `nix develop` works without devenv.
          default = pkgs.mkShell {
            packages = buildDeps pkgs ++ [
              pkgs.bun
              pkgs.cargo
              pkgs.clippy
              pkgs.curl
              pkgs.jq
              pkgs.openssh
              pkgs.rust-analyzer
              pkgs.rustc
              pkgs.rustfmt
            ];
          };
        });

      formatter = forAllSystems (system: nixpkgs.legacyPackages.${system}.nixpkgs-fmt);

      checks = forAllSystems (system:
        let pkgs = nixpkgs.legacyPackages.${system}; in
        {
          package = self.packages.${system}.gitadel;

          module = pkgs.testers.runNixOSTest {
            name = "gitadel-module";
            nodes.machine = {
              imports = [ self.nixosModules.gitadel ];
              environment.systemPackages = [ pkgs.curl pkgs.git ];
              services.gitadel = {
                enable = true;
                publicUrl = "http://localhost:3000";
                initialAdmin = {
                  username = "admin";
                  passwordFile = "/etc/gitadel-admin-password";
                };
              };
              environment.etc."gitadel-admin-password".text = "hunter2hunter2";
            };
            testScript = ''
              machine.wait_for_unit("gitadel.service")
              machine.wait_for_open_port(3000)
              machine.wait_for_open_port(2222)
              machine.succeed("curl -fsS http://127.0.0.1:3000/api/v1/healthz")
              # The unit must survive a restart even though bootstrapping the
              # administrator now fails because the account already exists.
              machine.succeed("systemctl restart gitadel.service")
              machine.wait_for_open_port(3000)
              machine.succeed("test -d /var/lib/gitadel/repositories")
              machine.succeed("test -f /var/lib/gitadel/ssh-host-ed25519")
            '';
          };
        });

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
