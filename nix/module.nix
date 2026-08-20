{ config, lib, pkgs, ... }:

let
  cfg = config.services.gitadel;
  toml = pkgs.formats.toml { };
  socketAddress = address: port:
    if lib.hasInfix ":" address
    then "[${address}]:${toString port}"
    else "${address}:${toString port}";
  generatedSettings = {
    server = {
      bind = socketAddress cfg.http.address cfg.http.port;
      public_url = cfg.publicUrl;
    };
    database.url = "sqlite://${cfg.dataDir}/gitadel.db?mode=rwc";
    storage = {
      repository_root = "${cfg.dataDir}/repositories";
      lfs_root = "${cfg.dataDir}/lfs";
    };
    ssh = {
      bind = socketAddress cfg.ssh.address cfg.ssh.port;
      host_key = "${cfg.dataDir}/ssh-host-ed25519";
    };
  };
  settingsFile = toml.generate "gitadel.toml" (lib.recursiveUpdate generatedSettings cfg.settings);
in
{
  options.services.gitadel = {
    enable = lib.mkEnableOption "Gitadel Git archive server";

    package = lib.mkOption {
      type = lib.types.package;
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
      type = lib.types.path;
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

    openFirewall = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Open the configured HTTP and SSH ports in the NixOS firewall.";
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

    systemd.tmpfiles.rules = [
      "d ${cfg.dataDir} 0750 ${cfg.user} ${cfg.group} -"
    ];

    systemd.services.gitadel = {
      description = "Gitadel Git archive server";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" ];
      serviceConfig = {
        Type = "simple";
        User = cfg.user;
        Group = cfg.group;
        WorkingDirectory = cfg.dataDir;
        ExecStart = "${lib.getExe cfg.package} --config ${settingsFile}";
        Restart = "on-failure";
        RestartSec = 2;
        UMask = "0027";
        NoNewPrivileges = true;
        PrivateTmp = true;
        ProtectHome = true;
        ProtectSystem = "strict";
        ReadWritePaths = [ cfg.dataDir ];
        CapabilityBoundingSet = "";
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        RestrictAddressFamilies = [ "AF_INET" "AF_INET6" "AF_UNIX" ];
        RestrictNamespaces = true;
        RestrictRealtime = true;
        SystemCallArchitectures = "native";
      };
      environment.PATH = lib.makeBinPath [ pkgs.git ];
    };

    networking.firewall.allowedTCPPorts = lib.mkIf cfg.openFirewall [
      cfg.http.port
      cfg.ssh.port
    ];
  };
}
