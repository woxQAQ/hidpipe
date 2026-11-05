{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.hidpipe;
  server = import ./server.nix { inherit lib pkgs cfg; };
  openlogi = import ./openlogi.nix {
    inherit lib pkgs cfg;
    homeDirectory = config.users.users.${cfg.user}.home;
  };
in
{
  options.services.hidpipe = server.options // {
    openlogi = openlogi.options;
    user = lib.mkOption {
      type = lib.types.str;
      example = "alice";
      description = ''
        Existing desktop user who runs the server. This user is added to the
        input group, which grants access to all input devices. Log in again
        after enabling the service for the group membership to take effect.
      '';
    };

    openFirewall = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Open the server TCP port in the firewall.";
    };
  };

  config = lib.mkIf cfg.enable {
    users.users.${cfg.user}.extraGroups = [ "input" ] ++ lib.optional cfg.openlogi.enable "uinput";
    environment.systemPackages = [
      cfg.package
    ]
    ++ lib.optional cfg.openlogi.enable cfg.openlogi.package;
    assertions = [
      {
        assertion = !cfg.clipboard.enable || cfg.clipboard.port != cfg.port;
        message = "services.hidpipe.clipboard.port must differ from services.hidpipe.port.";
      }
      {
        assertion = !cfg.openlogi.enable || cfg.mouse == null;
        message = "services.hidpipe.mouse must be null when OpenLogi owns mouse capture; use openlogi.mouse.";
      }
    ];
    hardware.uinput.enable = lib.mkIf cfg.openlogi.enable true;
    services.udev.packages = lib.optional cfg.openlogi.enable cfg.openlogi.package;
    networking.firewall.allowedTCPPorts = lib.optionals cfg.openFirewall (
      [ cfg.port ] ++ lib.optional cfg.clipboard.enable cfg.clipboard.port
    );

    systemd.services.openlogi-udev = lib.mkIf (cfg.openlogi.enable && config.services.udev.enable) {
      description = "Apply OpenLogi permissions to connected Logitech HID devices";
      wantedBy = [ "multi-user.target" ];
      requires = [ "systemd-udevd.service" ];
      after = [ "systemd-udevd.service" ];
      restartTriggers = [ config.environment.etc."udev/rules.d".source ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        ExecStart = "${config.systemd.package}/bin/udevadm trigger --action=change --subsystem-match=hidraw --property-match=ID_VENDOR_ID=046d --settle";
      };
    };

    systemd.user.services.openlogi-agent = lib.mkIf cfg.openlogi.enable {
      description = "OpenLogi input owner and hidpipe pointer bridge";
      after = [ cfg.systemdTarget ];
      partOf = [ cfg.systemdTarget ];
      wantedBy = [ cfg.systemdTarget ];
      unitConfig.ConditionUser = cfg.user;
      environment = openlogi.environment;
      serviceConfig = {
        ExecStart = "${cfg.openlogi.package}/bin/openlogi-agent";
        Restart = "on-failure";
        RestartSec = 2;
      };
    };

    systemd.user.services.hidpipe = {
      description = server.service.Unit.Description;
      after = server.service.Unit.After;
      partOf = server.service.Unit.PartOf;
      wants = server.service.Unit.Wants;
      wantedBy = server.service.Install.WantedBy;
      unitConfig.ConditionUser = cfg.user;
      serviceConfig = server.service.Service;
    };
  };
}
