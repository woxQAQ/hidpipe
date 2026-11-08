{ nixpkgs }:
let
  lib = nixpkgs.lib;

  # Only the surrounding HM containers are stubbed. These checks exercise the
  # real module's service output boundary, not a full Home Manager activation.
  containers = { lib, ... }: {
    options = {
      home.homeDirectory = lib.mkOption { type = lib.types.str; };
      home.packages = lib.mkOption {
        type = lib.types.listOf lib.types.package;
        default = [ ];
      };
      home.activation = lib.mkOption {
        type = lib.types.attrsOf lib.types.anything;
        default = { };
      };
      launchd.agents = lib.mkOption {
        type = lib.types.attrsOf lib.types.anything;
        default = { };
      };
      systemd.user.services = lib.mkOption {
        type = lib.types.attrsOf lib.types.anything;
        default = { };
      };
      assertions = lib.mkOption {
        type = lib.types.listOf lib.types.anything;
        default = [ ];
      };
    };
  };

  evaluate =
    system: settings:
    let
      pkgs = import nixpkgs { inherit system; };
    in
    (lib.evalModules {
      specialArgs = { inherit pkgs; };
      modules = [
        containers
        ../modules/home-manager.nix
        {
          home.homeDirectory = "/Users/scroll-test";
          services.hidpipe = {
            enable = true;
            host = "linux-host.local";
            package = pkgs.hello;
            openlogi.package = pkgs.hello;
            localSigning.enable = false;
          };
        }
        { services.hidpipe = settings; }
      ];
    }).config;

  darwin = evaluate "aarch64-darwin";
  defaults = darwin { };
  arguments =
    config:
    builtins.deepSeq config.assertions (
      if lib.all (entry: entry.assertion) config.assertions then
        config.launchd.agents.hidpipe.config.ProgramArguments
      else
        throw "hidpipe service assertions failed"
    );
  base = [
    (lib.getExe defaults.services.hidpipe.package)
    "linux-host.local"
    "--port"
    "24800"
  ];
  changedScroll = {
    scaleX = 0.5;
    scaleY = 2;
    invertX = true;
    invertY = true;
  };
  changedFlags = [
    "--scroll-scale-x"
    "0.5"
    "--scroll-scale-y"
    "2"
    "--invert-scroll-x"
    "--invert-scroll-y"
  ];
  # Assert exact argv strings as well as numeric round trips: toString rounds
  # floats to six decimal places and can silently turn a small scale into zero.
  scaleCases = [
    {
      value = 0.0000001;
      expected = "1e-07";
    }
    {
      value = 1.0e-300;
      expected = "1e-300";
    }
    {
      value = 0.12345678901234566;
      expected = "0.12345678901234566";
    }
    {
      value = 1.7976931348623157e308;
      expected = "1.7976931348623157e+308";
    }
    {
      value = 2.0;
      expected = "2.0";
    }
    {
      value = 9223372036854775807;
      expected = "9223372036854775807";
    }
  ];
  combined = darwin {
    scroll = changedScroll;
    openlogi.enable = true;
    clipboard.enable = true;
    inputFeedback = true;
    latencyStats = true;
  };
  linuxDefaults = evaluate "x86_64-linux" { };
  linuxChanged = evaluate "x86_64-linux" { scroll = changedScroll; };
  linuxExec = config: config.systemd.user.services.hidpipe.Service.ExecStart;
  rejected =
    scroll:
    !(builtins.tryEval (
      builtins.deepSeq (arguments (darwin {
        inherit scroll;
      })) true
    )).success;
  infinity = 1.7976931348623157e308 * 2;
  invalidScales = [
    (-1)
    (-0.5)
    "0.5"
    null
    true
    infinity
    (-infinity)
    (infinity - infinity)
  ];
  checks = {
    defaults =
      arguments defaults == base
      &&
        defaults.services.hidpipe.scroll == {
          scaleX = 1.0;
          scaleY = 1.0;
          invertX = false;
          invertY = false;
        }
      &&
        arguments (darwin {
          scroll = {
            scaleX = 1;
            scaleY = 1;
          };
        }) == base;
    changed =
      arguments (darwin {
        scroll = changedScroll;
      }) == base ++ changedFlags;
    serialization = lib.all (
      case:
      let
        argv = arguments (darwin {
          scroll = {
            scaleX = case.value;
            scaleY = case.value;
          };
        });
      in
      argv == base
      ++ [
        "--scroll-scale-x"
        case.expected
        "--scroll-scale-y"
        case.expected
      ]
      && builtins.fromJSON (builtins.elemAt argv 5) == case.value
      && builtins.fromJSON (builtins.elemAt argv 7) == case.value
    ) scaleCases;
    zero =
      arguments (darwin {
        scroll.scaleX = 0;
      }) == base
      ++ [
        "--scroll-scale-x"
        "0"
      ]
      &&
        arguments (darwin {
          scroll.scaleY = 0.0;
        }) == base
        ++ [
          "--scroll-scale-y"
          "0.0"
        ];
    invalid =
      lib.all (value: rejected { scaleX = value; }) invalidScales
      && lib.all (value: rejected { scaleY = value; }) invalidScales
      && lib.all (value: rejected { invertX = value; }) [
        0
        "true"
        null
      ]
      && lib.all (value: rejected { invertY = value; }) [
        0
        "true"
        null
      ];
    combined =
      arguments combined == base
      ++ [
        "--openlogi-socket"
        "/Users/scroll-test/.cache/openlogi-hidpipe/pointer.sock"
        "--clipboard"
        "--clipboard-port"
        "24801"
        "--input-feedback"
        "--latency-stats"
      ]
      ++ changedFlags
      &&
        combined.launchd.agents.openlogi-agent.config.ProgramArguments == [
          "${combined.services.hidpipe.openlogi.package}/bin/openlogi-agent"
        ];
    linux =
      linuxExec linuxDefaults == linuxExec linuxChanged
      && lib.hasInfix "--bind" (linuxExec linuxChanged)
      && !(lib.hasInfix "scroll" (linuxExec linuxChanged))
      && !(linuxChanged.launchd.agents ? hidpipe);
  };
in
builtins.deepSeq checks (
  lib.mapAttrs (
    name: passed: if passed then "PASS" else throw "scroll-options: ${name} failed"
  ) checks
)
