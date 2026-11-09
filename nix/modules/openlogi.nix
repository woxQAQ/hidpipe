{
  lib,
  pkgs,
  cfg,
  homeDirectory,
}:
{
  options = {
    enable = lib.mkEnableOption "OpenLogi pointer routing with destination-side bindings";
    gui.enable = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Build and install the OpenLogi GUI alongside the CLI and agent.
        This preserves the desktop launcher on Linux and the application bundle
        on macOS. Disable for an agent-only installation; this does not start
        a GUI automatically.
      '';
    };
    gestures = {
      repeatThreshold = lib.mkOption {
        type = lib.types.ints.between 1 32767;
        default = 150;
        description = ''
          Minimum raw-XY displacement for each subsequent gesture while holding
          the button. Higher values require more travel and reduce sensitivity.
          The first gesture retains OpenLogi's 50-unit threshold.
          Changing this option rebuilds the default OpenLogi package.
        '';
      };
      repeatIntervalMs = lib.mkOption {
        type = lib.types.ints.between 1 60000;
        default = 300;
        description = ''
          Minimum milliseconds between directional gesture actions in one hold.
          Higher values slow repetition. The first gesture retains OpenLogi's
          160 ms hold gate. Changing this option rebuilds the default OpenLogi
          package. A custom openlogi.package must provide its own gesture settings.
        '';
      };
    };
    basePackage = lib.mkOption {
      type = lib.types.package;
      default = pkgs.openlogi;
      description = "Unmodified OpenLogi 0.6.25 package to which the bridge patch is applied.";
    };
    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ../openlogi-package.nix {
        openlogi = cfg.openlogi.basePackage;
        withGui = cfg.openlogi.gui.enable;
        gestureRepeatThreshold = cfg.openlogi.gestures.repeatThreshold;
        gestureRepeatIntervalMs = cfg.openlogi.gestures.repeatIntervalMs;
      };
      description = "OpenLogi with the hidpipe bridge patches. GUI inclusion follows openlogi.gui.enable.";
    };
    socket = lib.mkOption {
      type = lib.types.str;
      default = "${homeDirectory}/.cache/openlogi-hidpipe/pointer.sock";
      description = "Private local socket used by hidpipe and the OpenLogi agent.";
    };
    device = lib.mkOption {
      type = lib.types.nullOr lib.types.nonEmptyStr;
      default = null;
      description = ''
        This machine's OpenLogi config key. On Linux, null selects the first
        online pointing device; on macOS it uses OpenLogi's saved selection.
        Set explicitly when multiple pointing devices are configured.
      '';
    };
    mouse = lib.mkOption {
      type = lib.types.nonEmptyStr;
      default = "MX Master";
      description = "Linux physical mouse name substring; must match exactly one Logitech mouse.";
    };
  };
  environment = {
    OPENLOGI_BRIDGE_SOCKET = cfg.openlogi.socket;
    OPENLOGI_MANAGED_SERVICE = "1";
  }
  // lib.optionalAttrs (cfg.openlogi.device != null) {
    OPENLOGI_BRIDGE_DEVICE = cfg.openlogi.device;
  }
  // lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
    OPENLOGI_BRIDGE_MOUSE = cfg.openlogi.mouse;
  };
}
