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
      After = lib.optional (cfg.systemdTarget != "default.target") cfg.systemdTarget;
      PartOf = [ cfg.systemdTarget ];
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
        ++ lib.optional cfg.swapCapsEsc "--swap-caps-esc"
      );
      Restart = "on-failure";
      RestartSec = 5;
    };
    Install.WantedBy = [ cfg.systemdTarget ];
  };
}
