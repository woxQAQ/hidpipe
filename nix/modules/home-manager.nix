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
in
{
  options.services.hidpipe = server.options // {
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

    flipScroll = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Reverse the scroll direction of injected mouse events on macOS.";
    };
  };

  config = lib.mkIf cfg.enable (
    lib.mkMerge [
      {
        home.packages = [ cfg.package ];
        assertions = [
          {
            assertion = !isDarwin || cfg.host != null;
            message = "services.hidpipe.host must be set on macOS.";
          }
        ];
      }

      (lib.mkIf (!isDarwin) {
        systemd.user.services.hidpipe = server.service;
      })

      (lib.mkIf isDarwin {
        launchd.agents.hidpipe = {
          enable = true;
          config = {
            ProgramArguments = [
              (lib.getExe cfg.package)
              cfg.host
              "--port"
              (toString cfg.port)
            ]
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
