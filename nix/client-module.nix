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
        Optional non-secret Gitadel server URL exported as GITADEL_SERVER for
        users of the installed CLI. Authentication tokens are not configured
        through NixOS options.
      '';
    };

    tokenFile = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "/run/secrets/gitadel_api_token";
      description = "Runtime token-file path, exported as GITADEL_TOKEN_FILE. Never put the token itself in Nix.";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];
    environment.variables =
      lib.optionalAttrs (cfg.serverUrl != null) { GITADEL_SERVER = cfg.serverUrl; }
      // lib.optionalAttrs (cfg.tokenFile != null) { GITADEL_TOKEN_FILE = cfg.tokenFile; };
  };
}
