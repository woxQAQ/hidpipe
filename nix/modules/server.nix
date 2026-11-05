{
  lib,
  pkgs,
  cfg,
}:
let
  utils = import (pkgs.path + "/nixos/lib/utils.nix") {
    inherit lib pkgs;
    config = { };
  };
in
{
  options = {
    enable = lib.mkEnableOption "hidpipe Linux input capture";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ../package.nix { };
      defaultText = lib.literalExpression "hidpipe.packages.\${pkgs.stdenv.hostPlatform.system}.hidpipe-server";
      description = "Package providing hidpipe-server.";
    };

    bind = lib.mkOption {
      type = lib.types.str;
      default = "0.0.0.0";
      description = "Address on which to listen for the macOS client.";
    };

    port = lib.mkOption {
      type = lib.types.port;
      default = 24800;
      description = "TCP port on which to listen.";
    };

    clipboard = {
      enable = lib.mkEnableOption "bidirectional text clipboard sharing over the trusted LAN";
      port = lib.mkOption {
        type = lib.types.port;
        default = 24801;
        description = "Separate TCP port for clipboard sharing; must differ from the input port.";
      };
    };

    keyboard = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "keyd";
      description = "Keyboard device name substring, or null for automatic detection.";
    };

    mouse = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "MX Master";
      description = "Mouse device name substring, or null for automatic detection.";
    };

    latencyStats = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Log local latency counts, means and maxima every 5 seconds.";
    };

    swapCapsEsc = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Swap Caps Lock and Escape on capture, when keyd does not already do so.";
    };

    systemdTarget = lib.mkOption {
      type = lib.types.str;
      default = "graphical-session.target";
      example = "sway-session.target";
      description = ''
        User target that starts and stops the server. For edge switching, the
        compositor must import WAYLAND_DISPLAY into the systemd user environment
        before starting this target.
      '';
    };
  };

  service = {
    Unit = {
      Description = "hidpipe input capture";
      After =
        lib.optional (cfg.systemdTarget != "default.target") cfg.systemdTarget
        ++ lib.optional cfg.openlogi.enable "openlogi-agent.service";
      PartOf = [ cfg.systemdTarget ];
      Wants = lib.optional cfg.openlogi.enable "openlogi-agent.service";
    };
    Service = {
      ExecStart = utils.escapeSystemdExecArgs (
        [
          (lib.getExe cfg.package)
          "--bind"
          cfg.bind
          "--port"
          (toString cfg.port)
        ]
        ++ lib.optionals (cfg.keyboard != null) [
          "--keyboard"
          cfg.keyboard
        ]
        ++ lib.optionals (cfg.mouse != null) [
          "--mouse"
          cfg.mouse
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
        ++ lib.optional cfg.latencyStats "--latency-stats"
        ++ lib.optional cfg.swapCapsEsc "--swap-caps-esc"
      );
      Restart = "on-failure";
      RestartSec = 5;
    };
    Install.WantedBy = [ cfg.systemdTarget ];
  };
}
