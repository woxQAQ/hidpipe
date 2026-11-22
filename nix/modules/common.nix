{ lib, pkgs, ... }:
{
  options.services.hidpipe = {
    enable = lib.mkEnableOption "hidpipe";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ../package.nix { };
      defaultText = lib.literalExpression "hidpipe.packages.\${pkgs.stdenv.hostPlatform.system}.default";
      description = "Package providing the server on Linux or the client on macOS.";
    };

    port = lib.mkOption {
      type = lib.types.port;
      default = 24800;
      description = "TCP port used by the server and client for input events.";
    };

    clipboard = {
      enable = lib.mkEnableOption "bidirectional text and image clipboard sharing over the trusted LAN";
      port = lib.mkOption {
        type = lib.types.port;
        default = 24801;
        description = "Separate TCP port for clipboard sharing. Must differ from the input port.";
      };
    };

    trace.endpoint = lib.mkOption {
      type = lib.types.nullOr lib.types.nonEmptyStr;
      default = null;
      example = "http://collector:4318/v1/traces";
      description = "OTLP/HTTP trace URL. Set on both peers to export sampled input traces.";
    };

    latencyStats = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Log local latency counts, means and maxima every 5 seconds.";
    };
  };
}
