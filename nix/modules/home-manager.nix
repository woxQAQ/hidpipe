{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.hidpipe;
  isDarwin = pkgs.stdenv.hostPlatform.isDarwin;
  server = import ./server.nix { inherit lib pkgs cfg; };
  signedApps = isDarwin && cfg.localSigning.enable;
  applicationsDirectory = "${config.home.homeDirectory}/Applications";
  signingDirectory = "${config.home.homeDirectory}/Library/Application Support/hidpipe/signing";
  clientExecutable =
    if signedApps then
      "${applicationsDirectory}/Hidpipe.app/Contents/MacOS/hidpipe-client"
    else
      lib.getExe cfg.package;
  agentExecutable =
    if signedApps then
      "${applicationsDirectory}/OpenLogi Agent.app/Contents/MacOS/openlogi-agent"
    else
      "${cfg.openlogi.package}/bin/openlogi-agent";
  openlogi = import ./openlogi.nix {
    inherit lib pkgs cfg;
    homeDirectory = config.home.homeDirectory;
  };
in
{
  options.services.hidpipe = server.options // {
    openlogi = openlogi.options;
    enable = lib.mkEnableOption "hidpipe";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ../package.nix { };
      defaultText = lib.literalExpression "hidpipe.packages.\${pkgs.stdenv.hostPlatform.system}.default";
      description = "Package providing the server on Linux or the client on macOS.";
    };

    localSigning.enable =
      lib.mkEnableOption "persistent local signing and stable macOS application bundles"
      // {
        default = isDarwin;
        description = ''
          Install Hidpipe.app and, when enabled, OpenLogi Agent.app in ~/Applications.
          Sign both with a persistent identity held in a private local Keychain so
          rebuilt programs retain the same code identity. Initial Accessibility
          authorization is still required. The signing state is stored in
          ~/Library/Application Support/hidpipe/signing and must survive upgrades.
        '';
      };

    host = lib.mkOption {
      type = lib.types.nullOr lib.types.nonEmptyStr;
      default = null;
      example = "192.168.1.10";
      description = "Host name or IP address of the Linux capture server. Required on macOS.";
    };

    port = lib.mkOption {
      type = lib.types.port;
      default = 24800;
      description = "TCP port used by the server and client.";
    };

    inputFeedback = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        Reply to each input frame on macOS to mitigate TCP delayed ACK.
        Enable for comparison with the default behavior; adds a Pong per input frame.
      '';
    };
  };

  config = lib.mkIf cfg.enable (
    lib.mkMerge [
      {
        home.packages = [ cfg.package ] ++ lib.optional cfg.openlogi.enable cfg.openlogi.package;
        assertions = [
          {
            assertion = !isDarwin || cfg.host != null;
            message = "services.hidpipe.host must be set on macOS.";
          }
          {
            assertion = !cfg.clipboard.enable || cfg.clipboard.port != cfg.port;
            message = "services.hidpipe.clipboard.port must differ from services.hidpipe.port.";
          }
          {
            assertion = !cfg.openlogi.enable || cfg.mouse == null;
            message = "Use services.hidpipe.openlogi.mouse when OpenLogi owns mouse capture.";
          }
        ];
      }

      (lib.mkIf (!isDarwin) {
        systemd.user.services.hidpipe = server.service;
        systemd.user.services.openlogi-agent = lib.mkIf cfg.openlogi.enable {
          Unit = {
            Description = "OpenLogi input owner and hidpipe pointer bridge";
            After = [ cfg.systemdTarget ];
            PartOf = [ cfg.systemdTarget ];
          };
          Service = {
            ExecStart = "${cfg.openlogi.package}/bin/openlogi-agent";
            Environment = lib.mapAttrsToList (name: value: "${name}=${value}") openlogi.environment;
            Restart = "on-failure";
            RestartSec = 2;
          };
          Install.WantedBy = [ cfg.systemdTarget ];
        };
      })

      (lib.mkIf isDarwin {
        # Install before launchd starts the jobs. Store paths in the job
        # environments also make Home Manager reload them after package updates.
        home.activation.hidpipeSignedApps = lib.mkIf signedApps (
          lib.hm.dag.entryBetween [ "setupLaunchAgents" ] [ "writeBoundary" ] ''
            run env OPENSSL=${lib.getExe pkgs.openssl} \
              ${pkgs.bash}/bin/bash ${../macos/install-apps.sh} \
              ${lib.escapeShellArg signingDirectory} \
              ${lib.escapeShellArg applicationsDirectory} \
              ${lib.escapeShellArg (lib.getExe cfg.package)} \
              ${lib.escapeShellArg (
                if cfg.openlogi.enable then "${cfg.openlogi.package}/bin/openlogi-agent" else ""
              )}
          ''
        );

        launchd.agents.openlogi-agent = lib.mkIf cfg.openlogi.enable {
          enable = true;
          config = {
            ProgramArguments = [ agentExecutable ];
            EnvironmentVariables =
              openlogi.environment
              // lib.optionalAttrs signedApps {
                HIDPIPE_SOURCE_PACKAGE = toString cfg.openlogi.package;
              };
            RunAtLoad = true;
            KeepAlive = true;
            ProcessType = "Interactive";
            LimitLoadToSessionType = "Aqua";
            StandardOutPath = "${config.home.homeDirectory}/Library/Logs/openlogi-agent.log";
            StandardErrorPath = "${config.home.homeDirectory}/Library/Logs/openlogi-agent.log";
          };
        };
        launchd.agents.hidpipe = {
          enable = true;
          config = {
            ProgramArguments = [
              clientExecutable
              cfg.host
              "--port"
              (toString cfg.port)
            ]
            ++ lib.optionals cfg.openlogi.enable [
              "--openlogi-socket"
              cfg.openlogi.socket
            ]
            ++ lib.optionals cfg.clipboard.enable [
              "--clipboard"
              "--clipboard-port"
              (toString cfg.clipboard.port)
            ]
            ++ lib.optional cfg.inputFeedback "--input-feedback"
            ++ lib.optional cfg.latencyStats "--latency-stats";
            EnvironmentVariables = lib.optionalAttrs signedApps {
              HIDPIPE_SOURCE_PACKAGE = toString cfg.package;
            };
            RunAtLoad = true;
            KeepAlive = true;
            ProcessType = "Interactive";
            LimitLoadToSessionType = "Aqua";
            StandardOutPath = "${config.home.homeDirectory}/Library/Logs/hidpipe.log";
            StandardErrorPath = "${config.home.homeDirectory}/Library/Logs/hidpipe.log";
          };
        };
      })
    ]
  );
}
