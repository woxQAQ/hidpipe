{
  lib,
  stdenv,
  openlogi,
  withGui ? true,
}:
assert lib.assertMsg (
  openlogi.version == "0.6.25"
) "The hidpipe bridge patches target OpenLogi 0.6.25.";
openlogi.overrideAttrs (
  old:
  {
    patches = (old.patches or [ ]) ++ [
      ./openlogi-config-symlink.patch
      ./openlogi-pointer-capture.patch
      ./openlogi-bridge.patch
      ./openlogi-switch-recovery.patch
      ./openlogi-managed-restart.patch
    ];
    postPatch =
      (old.postPatch or "")
      + ''
        # Both processes compile the same fixed-size pointer protocol.
        install -Dm644 ${../proto/src/pointer.rs} crates/openlogi-agent-core/src/remote/wire.rs
      ''
      + lib.optionalString stdenv.hostPlatform.isLinux ''
        install -Dm644 ${./tests/openlogi-capture.rs} crates/openlogi-hook/examples/hidpipe-capture.rs
        install -Dm644 ${./tests/openlogi-switch-recovery.rs} crates/openlogi-agent-core/tests/switch_recovery.rs
      '';
    passthru = (old.passthru or { }) // {
      inherit withGui;
    };
  }
  // lib.optionalAttrs (!withGui) {
    cargoBuildFlags = builtins.filter (flag: flag != "--package=openlogi-gui") old.cargoBuildFlags;
    cargoTestFlags = [
      "--workspace"
      "--exclude=openlogi-gui"
      "--exclude=xtask"
    ];
    buildFeatures = builtins.filter (feature: feature != "gpui_platform/runtime_shaders") (
      old.buildFeatures or [ ]
    );
    env = builtins.removeAttrs (old.env or { }) [ "OPENLOGI_THEMES_DIR" ];
    nativeBuildInputs = builtins.filter (input: lib.getName input != "cargo-bundle") (
      old.nativeBuildInputs or [ ]
    );
    installPhase = ''
      runHook preInstall

      release_target="target/${stdenv.hostPlatform.rust.cargoShortTarget}/release"
      install -Dm755 "$release_target/openlogi" "$out/bin/openlogi"
      install -Dm755 "$release_target/openlogi-agent" "$out/bin/openlogi-agent"
    ''
    + lib.optionalString stdenv.hostPlatform.isLinux ''
      install -Dm644 packaging/linux/udev/70-openlogi.rules \
        "$out/lib/udev/rules.d/70-openlogi.rules"
      install -Dm644 packaging/linux/systemd/openlogi-agent.service \
        "$out/share/systemd/user/openlogi-agent.service"
      substituteInPlace "$out/share/systemd/user/openlogi-agent.service" \
        --replace-fail "ExecStart=/usr/bin/openlogi-agent" \
                       "ExecStart=$out/bin/openlogi-agent"
    ''
    + ''
      runHook postInstall
    '';
    postFixup = "";
  }
)
