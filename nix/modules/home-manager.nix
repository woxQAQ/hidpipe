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

    scrollPixelsPerNotch = lib.mkOption {
      type = lib.types.ints.between 1 65535;
      default = 15;
      example = 10;
      description = ''
        Pixels per wheel notch for OpenLogi mouse input on macOS, on both axes.
        Lower values reduce scroll speed. Native evdev input is unaffected.
      '';
    };

    flipScroll = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Reverse the scroll direction of injected mouse events on macOS.";
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
        launchd.agents.openlogi-agent = lib.mkIf cfg.openlogi.enable {
          enable = true;
          config = {
            ProgramArguments = [ "${cfg.openlogi.package}/bin/openlogi-agent" ];
            EnvironmentVariables = openlogi.environment;
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
              (lib.getExe cfg.package)
              cfg.host
              "--port"
              (toString cfg.port)
              "--scroll-pixels-per-notch"
              (toString cfg.scrollPixelsPerNotch)
            ]
            ++ lib.optionals cfg.openlogi.enable [
              "--openlogi-socket"
              cfg.openlogi.socket
            ]
            ++ lib.optional cfg.inputFeedback "--input-feedback"
            ++ lib.optional cfg.latencyStats "--latency-stats"
            ++ lib.optional cfg.flipScroll "--flip-scroll";
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
