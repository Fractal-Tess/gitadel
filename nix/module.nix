{ config, lib, pkgs, ... }:

let
  cfg = config.services.gitadel;
  toml = pkgs.formats.toml { };

  socketAddress = address: port:
    if lib.hasInfix ":" address
    then "[${address}]:${toString port}"
    else "${address}:${toString port}";

  isPrivileged = port: port > 0 && port < 1024;

  capabilities =
    lib.optional (isPrivileged cfg.http.port || isPrivileged cfg.ssh.port)
      "CAP_NET_BIND_SERVICE";

  # systemd manages /var/lib/<name> itself, which is both cheaper and more
  # correct than tmpfiles. Fall back to tmpfiles for data directories elsewhere.
  stateDirectory =
    if lib.hasPrefix "/var/lib/" cfg.dataDir && cfg.dataDir != "/var/lib/"
    then lib.removePrefix "/var/lib/" cfg.dataDir
    else null;

  generatedSettings = {
    server = {
      bind = socketAddress cfg.http.address cfg.http.port;
      public_url = cfg.publicUrl;
    };
    database.url = cfg.database.url;
    storage = {
      repository_root = "${cfg.dataDir}/repositories";
      lfs_root = "${cfg.dataDir}/lfs";
    };
    ssh = {
      bind = socketAddress cfg.ssh.address cfg.ssh.port;
      host_key = "${cfg.dataDir}/ssh-host-ed25519";
    };
    auth = {
      session_lifetime_hours = cfg.auth.sessionLifetimeHours;
      invitation_lifetime_hours = cfg.auth.invitationLifetimeHours;
    };
  };

  settingsFile = toml.generate "gitadel.toml" (lib.recursiveUpdate generatedSettings cfg.settings);

  # Exits non-zero once an account exists, so the unit ignores its failure.
  bootstrapAdmin = pkgs.writeShellScript "gitadel-bootstrap-admin" ''
    exec ${lib.getExe cfg.package} --config ${settingsFile} \
      --bootstrap-admin ${lib.escapeShellArg cfg.initialAdmin.username} \
      --password-stdin < ${lib.escapeShellArg (toString cfg.initialAdmin.passwordFile)}
  '';
in
{
  options.services.gitadel = {
    enable = lib.mkEnableOption "Gitadel Git archive server";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.gitadel;
      defaultText = lib.literalExpression "pkgs.gitadel";
      description = "Gitadel package to run.";
    };

    user = lib.mkOption {
      type = lib.types.str;
      default = "gitadel";
      description = "System user that runs Gitadel.";
    };

    group = lib.mkOption {
      type = lib.types.str;
      default = "gitadel";
      description = "System group that runs Gitadel.";
    };

    dataDir = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/gitadel";
      description = "Persistent database, repository, LFS, and SSH key directory.";
    };

    publicUrl = lib.mkOption {
      type = lib.types.str;
      default = "http://localhost:3000";
      description = "Public browser origin used for clone links, cookies, and passkeys.";
    };

    http = {
      address = lib.mkOption {
        type = lib.types.str;
        default = "127.0.0.1";
        description = "HTTP listen address.";
      };

      port = lib.mkOption {
        type = lib.types.port;
        default = 3000;
        description = "HTTP listen port.";
      };
    };

    ssh = {
      address = lib.mkOption {
        type = lib.types.str;
        default = "0.0.0.0";
        description = "SSH listen address.";
      };

      port = lib.mkOption {
        type = lib.types.port;
        default = 2222;
        description = "SSH listen port.";
      };
    };

    database.url = lib.mkOption {
      type = lib.types.str;
      default = "sqlite://${cfg.dataDir}/gitadel.db?mode=rwc";
      defaultText = lib.literalExpression ''"sqlite://''${config.services.gitadel.dataDir}/gitadel.db?mode=rwc"'';
      example = "postgres://gitadel@localhost/gitadel";
      description = ''
        SeaORM database URL. This value ends up in a world-readable
        {file}`/nix/store` file, so pass credentials through
        {option}`services.gitadel.environmentFile` instead of embedding them here.
      '';
    };

    auth = {
      sessionLifetimeHours = lib.mkOption {
        type = lib.types.ints.positive;
        default = 24 * 30;
        description = "How long a browser session stays valid.";
      };

      invitationLifetimeHours = lib.mkOption {
        type = lib.types.ints.positive;
        default = 72;
        description = "How long an unredeemed invitation stays valid.";
      };
    };

    openFirewall = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Open the configured HTTP and SSH ports in the NixOS firewall.";
    };

    environmentFile = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = null;
      example = "/run/secrets/gitadel.env";
      description = ''
        Path to a systemd `EnvironmentFile` holding secrets, read at service
        start rather than at evaluation time. Keys use the
        `GITADEL__SECTION__KEY` form and override {option}`settings`, for example
        `GITADEL__DATABASE__URL=postgres://gitadel:secret@localhost/gitadel`.
      '';
    };

    initialAdmin = lib.mkOption {
      default = null;
      description = ''
        Create the first administrator on first start. This is a no-op once any
        account exists, so the credentials can safely stay in the configuration.
      '';
      type = lib.types.nullOr (lib.types.submodule {
        options = {
          username = lib.mkOption {
            type = lib.types.str;
            description = "Username of the first administrator.";
          };

          passwordFile = lib.mkOption {
            type = lib.types.path;
            description = ''
              File containing the initial administrator password. It must be
              readable by {option}`services.gitadel.user` and cannot live under
              {file}`/home` or {file}`/root`, which the unit hides.
            '';
          };
        };
      });
    };

    settings = lib.mkOption {
      inherit (toml) type;
      default = { };
      description = "Additional Gitadel TOML settings merged over module defaults.";
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = lib.hasPrefix "http://" cfg.publicUrl || lib.hasPrefix "https://" cfg.publicUrl;
        message = "services.gitadel.publicUrl must use http:// or https://";
      }
      {
        assertion = lib.hasPrefix "/" cfg.dataDir;
        message = "services.gitadel.dataDir must be an absolute path";
      }
    ];

    users.groups = lib.mkIf (cfg.group == "gitadel") {
      gitadel = { };
    };

    users.users = lib.mkIf (cfg.user == "gitadel") {
      gitadel = {
        inherit (cfg) group;
        isSystemUser = true;
        home = cfg.dataDir;
      };
    };

    systemd.tmpfiles.rules = lib.optional (stateDirectory == null)
      "d ${cfg.dataDir} 0750 ${cfg.user} ${cfg.group} -";

    systemd.services.gitadel = {
      description = "Gitadel Git archive server";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" ];
      path = [ pkgs.git ];
      serviceConfig = {
        Type = "simple";
        User = cfg.user;
        Group = cfg.group;
        WorkingDirectory = cfg.dataDir;
        ExecStartPre = lib.optional (cfg.initialAdmin != null) "-${bootstrapAdmin}";
        ExecStart = "${lib.getExe cfg.package} --config ${settingsFile}";
        EnvironmentFile = lib.mkIf (cfg.environmentFile != null) [ cfg.environmentFile ];
        Restart = "on-failure";
        RestartSec = 2;
        UMask = "0027";
        StateDirectory = lib.mkIf (stateDirectory != null) stateDirectory;
        StateDirectoryMode = lib.mkIf (stateDirectory != null) "0750";
        ReadWritePaths = lib.mkIf (stateDirectory == null) [ cfg.dataDir ];
        NoNewPrivileges = true;
        PrivateTmp = true;
        ProtectHome = true;
        ProtectSystem = "strict";
        AmbientCapabilities = capabilities;
        CapabilityBoundingSet = capabilities;
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        RestrictAddressFamilies = [ "AF_INET" "AF_INET6" "AF_UNIX" ];
        RestrictNamespaces = true;
        RestrictRealtime = true;
        SystemCallArchitectures = "native";
      };
    };

    networking.firewall.allowedTCPPorts = lib.mkIf cfg.openFirewall [
      cfg.http.port
      cfg.ssh.port
    ];
  };
}
