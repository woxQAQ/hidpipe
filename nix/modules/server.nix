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
    bind = lib.mkOption {
      type = lib.types.str;
      default = "0.0.0.0";
      description = "Address on which to listen for the macOS client.";
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

    swapCapsEsc = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Swap Caps Lock and Escape on capture, when keyd does not already do so.";
    };

    trace.sampleEvery = lib.mkOption {
      type = lib.types.ints.between 1 4294967295;
      default = 100;
      description = "Trace the first input frame and every Nth input thereafter on each trace-capable connection.";
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
        ++ lib.optionals (cfg.trace.endpoint != null) [
          "--trace-endpoint"
          cfg.trace.endpoint
          "--trace-every"
          (toString cfg.trace.sampleEvery)
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
