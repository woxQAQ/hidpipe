{
  lib,
  pkgs,
  cfg,
  homeDirectory,
}:
{
  options = {
    enable = lib.mkEnableOption "OpenLogi pointer routing with destination-side bindings";
    basePackage = lib.mkOption {
      type = lib.types.package;
      default = pkgs.openlogi;
      description = "Unmodified OpenLogi 0.6.25 package to which the bridge patch is applied.";
    };
    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ../openlogi-package.nix { openlogi = cfg.openlogi.basePackage; };
      description = "OpenLogi with the hidpipe bridge patch, including its GUI and agent.";
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
