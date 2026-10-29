{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.hidpipe;
  server = import ./server.nix { inherit lib pkgs cfg; };
in
{
  options.services.hidpipe = server.options // {
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
    users.users.${cfg.user}.extraGroups = [ "input" ];
    environment.systemPackages = [ cfg.package ];
    networking.firewall.allowedTCPPorts = lib.optional cfg.openFirewall cfg.port;

    systemd.user.services.hidpipe = {
      description = server.service.Unit.Description;
      after = server.service.Unit.After;
      partOf = server.service.Unit.PartOf;
      wantedBy = server.service.Install.WantedBy;
      unitConfig.ConditionUser = cfg.user;
      serviceConfig = server.service.Service;
    };
  };
}
