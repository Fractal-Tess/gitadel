{ self }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.programs.gitadel-cli;
  system = pkgs.stdenv.hostPlatform.system;
  defaultEnvironment =
    lib.optionalAttrs (cfg.serverUrl != null) { GITADEL_SERVER = cfg.serverUrl; }
    // lib.optionalAttrs (cfg.tokenFile != null) { GITADEL_TOKEN_FILE = cfg.tokenFile; };
  package =
    if defaultEnvironment == { } then
      cfg.package
    else
      pkgs.symlinkJoin {
        name = "${cfg.package.name}-with-defaults";
        paths = [ cfg.package ];
        nativeBuildInputs = [ pkgs.makeWrapper ];
        postBuild = ''
          wrapProgram "$out/bin/gtd" ${
            lib.concatStringsSep " " (
              lib.mapAttrsToList (
                name: value: "--set-default ${name} ${lib.escapeShellArg value}"
              ) defaultEnvironment
            )
          }
        '';
        meta = (cfg.package.meta or { }) // {
          mainProgram = "gtd";
        };
        passthru = cfg.package.passthru or { };
      };
in
{
  options.programs.gitadel-cli = {
    enable = lib.mkEnableOption "the Gitadel token-authenticated command-line client";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${system}.gitadel-cli;
      defaultText = lib.literalExpression "inputs.gitadel.packages.${pkgs.system}.gitadel-cli";
      description = "Gitadel CLI package to install.";
    };

    serverUrl = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "http://neo.netbird.cloud:3030";
      description = ''
        Optional non-secret Gitadel server URL supplied as the invocation
        default GITADEL_SERVER for users of the installed CLI. Inherited
        environment values and explicit command-line flags take precedence.
        Authentication tokens are not configured through NixOS options.
      '';
    };

    tokenFile = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "/run/secrets/gitadel_api_token";
      description = "Runtime token-file path supplied as the invocation default GITADEL_TOKEN_FILE. Never put the token itself in Nix.";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ package ];
  };
}
