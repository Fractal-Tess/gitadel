{
  description = "Gitadel archival Git server";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      packageFor = system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          nodeModules = pkgs.stdenv.mkDerivation {
            pname = "gitadel-frontend-node-modules";
            version = "0.1.0";
            src = ./frontend;
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
            outputHash = "sha256-3pgcCMpcREMBQzIoTgZFKedM+y9kej+bug4iKEioTwU=";
            outputHashAlgo = "sha256";
            outputHashMode = "recursive";
          };
          frontend = pkgs.stdenv.mkDerivation {
            pname = "gitadel-frontend";
            version = "0.1.0";
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
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = [
            pkgs.cmake
            pkgs.makeWrapper
            pkgs.perl
            pkgs.pkg-config
          ];
          preBuild = ''
            rm -rf frontend/build
            cp -R ${frontend} frontend/build
          '';
          postInstall = ''
            wrapProgram $out/bin/gitadel \
              --prefix PATH : ${nixpkgs.lib.makeBinPath [ pkgs.git ]}
          '';
          meta = {
            description = "A minimal self-hosted Git server for archival repositories";
            homepage = "https://github.com/Fractal-Tess/gitadel";
            license = nixpkgs.lib.licenses.mit;
            mainProgram = "gitadel";
            platforms = nixpkgs.lib.platforms.linux;
          };
        };
    in
    {
      packages = forAllSystems (system: {
        default = packageFor system;
        gitadel = packageFor system;
      });

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/gitadel";
        };
      });

      checks = forAllSystems (system: {
        package = self.packages.${system}.default;
      });

      overlays.default = final: _prev: {
        gitadel = self.packages.${final.system}.default;
      };

      nixosModules.default = {
        imports = [ ./nix/module.nix ];
        nixpkgs.overlays = [ self.overlays.default ];
      };
    };
}
