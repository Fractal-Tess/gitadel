{ self }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.gitadel;
  system = pkgs.stdenv.hostPlatform.system;
  toml = pkgs.formats.toml { };
  yaml = pkgs.formats.yaml { };
  runnerDataDir = "${cfg.dataDir}-runner";

  socketAddress =
    address: port:
    if lib.hasInfix ":" address then "[${address}]:${toString port}" else "${address}:${toString port}";

  isPrivileged = port: port > 0 && port < 1024;

  capabilities = lib.optional (
    isPrivileged cfg.http.port || isPrivileged cfg.ssh.port
  ) "CAP_NET_BIND_SERVICE";

  # systemd manages /var/lib/<name> itself, which is both cheaper and more
  # correct than tmpfiles. Fall back to tmpfiles for data directories elsewhere.
  stateDirectory =
    if lib.hasPrefix "/var/lib/" cfg.dataDir && cfg.dataDir != "/var/lib/" then
      lib.removePrefix "/var/lib/" cfg.dataDir
    else
      null;

  generatedSettings = {
    server = {
      bind = socketAddress cfg.http.address cfg.http.port;
      public_url = cfg.publicUrl;
    };
    database.url = cfg.database.url;
    storage = {
      repository_root = "${cfg.dataDir}/repositories";
      lfs_root = "${cfg.dataDir}/lfs";
      actions_artifact_root = "${cfg.dataDir}/actions-artifacts";
    };
    ssh = {
      bind = socketAddress cfg.ssh.address cfg.ssh.port;
      host_key = "${cfg.dataDir}/ssh-host-ed25519";
    };
    auth = {
      session_lifetime_hours = cfg.auth.sessionLifetimeHours;
      invitation_lifetime_hours = cfg.auth.invitationLifetimeHours;
    };
  }
  // lib.optionalAttrs cfg.runner.enable {
    actions.system_runner = {
      inherit (cfg.runner) name labels;
      registration_token_file = "${runnerDataDir}/registration-token";
    };
  };

  settingsFile = toml.generate "gitadel.toml" (lib.recursiveUpdate generatedSettings cfg.settings);

  runnerConfig = yaml.generate "gitadel-runner.yml" {
    runner = {
      file = ".runner";
      capacity = 1;
      timeout = "3h";
      shutdown_timeout = "3h";
      labels = lib.genAttrs cfg.runner.labels (_: {
        backend = "docker";
        backend-options.image = cfg.runner.jobImage;
      });
    };
    cache.enabled = false;
    container = {
      privileged = false;
      valid_volumes = [ ];
      docker_host = "-";
      force_pull = true;
    };
  };

  runnerStart = pkgs.writeShellScript "gitadel-runner-start" ''
    set -eu
    uid="$(${pkgs.coreutils}/bin/id -u ${lib.escapeShellArg cfg.user})"
    gid="$(${pkgs.coreutils}/bin/id -g ${lib.escapeShellArg cfg.user})"
    if [ ! -s ${lib.escapeShellArg "${runnerDataDir}/.runner"} ]; then
      until [ -s ${lib.escapeShellArg "${runnerDataDir}/registration-token"} ]; do
        sleep 1
      done
      ${pkgs.docker}/bin/docker run --rm \
        --user "$uid:$gid" \
        --network host \
        --volume ${lib.escapeShellArg "${runnerDataDir}:/data"} \
        ${lib.escapeShellArg cfg.runner.image} \
        forgejo-runner register --no-interactive \
          --instance ${lib.escapeShellArg cfg.publicUrl} \
          --token "$(${pkgs.coreutils}/bin/cat ${lib.escapeShellArg "${runnerDataDir}/registration-token"})" \
          --name ${lib.escapeShellArg cfg.runner.name} \
          --labels ${lib.escapeShellArg (lib.concatStringsSep "," cfg.runner.labels)}
      ${pkgs.coreutils}/bin/rm -f ${lib.escapeShellArg "${runnerDataDir}/registration-token"}
    fi
    exec ${pkgs.docker}/bin/docker run --rm \
      --name gitadel-runner \
      --user "$uid:$gid" \
      --network host \
      --env DOCKER_HOST=tcp://127.0.0.1:2375 \
      --volume ${lib.escapeShellArg "${runnerDataDir}:/data"} \
      --volume ${lib.escapeShellArg "${runnerConfig}:/data/config.yml:ro"} \
      ${lib.escapeShellArg cfg.runner.image} \
      forgejo-runner daemon --config /data/config.yml
  '';

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

    autoStart = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Start Gitadel and enabled runner units automatically at boot. When false, retain them for manual starts.";
    };

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${system}.gitadel;
      defaultText = lib.literalExpression "inputs.gitadel.packages.${pkgs.system}.gitadel";
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

    runner = {
      enable = lib.mkEnableOption "an automatically registered system Forgejo Runner";

      name = lib.mkOption {
        type = lib.types.str;
        default = "gitadel-system";
        description = "Name of the system runner shown in Gitadel.";
      };

      labels = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ "docker" ];
        description = "Scheduling labels exposed by the system runner.";
      };

      image = lib.mkOption {
        type = lib.types.str;
        default = "data.forgejo.org/forgejo/runner@sha256:7fb853bfe73c229be6349398359c0a7bd01fadfd17c106607b2221150b799ed2";
        description = "Forgejo Runner v13.0.0 OCI image.";
      };

      dockerImage = lib.mkOption {
        type = lib.types.str;
        default = "docker.io/library/docker@sha256:3ef33f2e220b79ed3ef3b99d81746f06f306cd6340e2cb7331d17ae996e74cb6";
        description = "Isolated Docker daemon image used for workflow containers.";
      };

      memoryBytes = lib.mkOption {
        type = lib.types.ints.positive;
        default = 4 * 1024 * 1024 * 1024;
        description = "Memory limit for the isolated workflow Docker daemon.";
      };

      jobImage = lib.mkOption {
        type = lib.types.str;
        default = "docker.io/library/node@sha256:8a34c4ab3ea2c5cd194f07e317b2a8f09461d3c8b05c4e34c8ccd56d56024c4d";
        description = "Default immutable container image for runner labels.";
      };
    };

    openFirewall = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Open the configured HTTP and SSH ports in the NixOS firewall.";
    };

    environment = lib.mkOption {
      type = lib.types.attrsOf lib.types.str;
      default = { };
      description = ''
        Additional non-secret environment variables for Gitadel. Values are
        written to the Nix store; use environmentFile for credentials.
      '';
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
      type = lib.types.nullOr (
        lib.types.submodule {
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
        }
      );
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
      {
        assertion = !cfg.runner.enable || cfg.runner.labels != [ ];
        message = "services.gitadel.runner.labels must contain at least one label";
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

    systemd.tmpfiles.rules =
      lib.optional (stateDirectory == null) "d ${cfg.dataDir} 0750 ${cfg.user} ${cfg.group} -"
      ++ lib.optional cfg.runner.enable "d ${runnerDataDir} 0700 ${cfg.user} ${cfg.group} -";

    systemd.services.gitadel = {
      description = "Gitadel Git archive server";
      wantedBy = lib.optional cfg.autoStart "multi-user.target";
      after = [ "network.target" ];
      path = [ pkgs.git ];
      environment = cfg.environment;
      serviceConfig = {
        Type = "simple";
        User = cfg.user;
        Group = cfg.group;
        WorkingDirectory = cfg.dataDir;
        ExecStartPre = lib.optional (cfg.initialAdmin != null) "-${bootstrapAdmin}";
        ExecStart = "${lib.getExe cfg.package} --config ${settingsFile}";
        EnvironmentFile = lib.mkIf (cfg.environmentFile != null) [ cfg.environmentFile ];
        RestartSec = 2;
        UMask = "0027";
        StateDirectory = lib.mkIf (stateDirectory != null) stateDirectory;
        StateDirectoryMode = lib.mkIf (stateDirectory != null) "0750";
        ReadWritePaths =
          lib.optional (stateDirectory == null) cfg.dataDir ++ lib.optional cfg.runner.enable runnerDataDir;
        NoNewPrivileges = true;
        PrivateTmp = true;
        ProtectHome = true;
        ProtectSystem = "strict";
        AmbientCapabilities = capabilities;
        CapabilityBoundingSet = capabilities;
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        RestrictAddressFamilies = [
          "AF_INET"
          "AF_INET6"
          "AF_UNIX"
        ];
        RestrictNamespaces = true;
        RestrictRealtime = true;
        SystemCallArchitectures = "native";
      };
    };

    virtualisation.docker.enable = lib.mkIf cfg.runner.enable true;

    systemd.services.gitadel-runner-docker = lib.mkIf cfg.runner.enable {
      description = "Isolated Docker daemon for Gitadel Actions";
      wantedBy = lib.optional cfg.autoStart "multi-user.target";
      after = [
        "docker.service"
        "network-online.target"
      ];
      serviceConfig = {
        ExecStartPre = [
          "-${pkgs.docker}/bin/docker rm -f gitadel-runner-docker"
          "${pkgs.docker}/bin/docker pull ${cfg.runner.dockerImage}"
        ];
        ExecStart = ''
          ${pkgs.docker}/bin/docker run --rm --name gitadel-runner-docker \
            --network host --privileged \
            --memory ${toString cfg.runner.memoryBytes} \
            --volume gitadel-runner-docker:/var/lib/docker \
            dockerd -H tcp://127.0.0.1:2375 --tls=false
        '';
        ExecStop = "-${pkgs.docker}/bin/docker stop gitadel-runner-docker";
        Restart = "on-failure";
        RestartSec = 2;
      };
    };

    systemd.services.gitadel-runner = lib.mkIf cfg.runner.enable {
      description = "Gitadel system Forgejo Runner";
      wantedBy = lib.optional cfg.autoStart "multi-user.target";
      after = [
        "gitadel.service"
        "gitadel-runner-docker.service"
        "network-online.target"
      ];
      serviceConfig = {
        ExecStartPre = [
          "-${pkgs.docker}/bin/docker rm -f gitadel-runner"
          "${pkgs.docker}/bin/docker pull ${cfg.runner.image}"
        ];
        ExecStart = runnerStart;
        ExecStop = "-${pkgs.docker}/bin/docker stop gitadel-runner";
        Restart = "on-failure";
        RestartSec = 2;
      };
    };

    networking.firewall.allowedTCPPorts = lib.mkIf cfg.openFirewall [
      cfg.http.port
      cfg.ssh.port
    ];
  };
}
