{
  lib,
  stdenv,
  openlogi,
  withGui ? true,
  gestureRepeatThreshold ? 150,
  gestureRepeatIntervalMs ? 300,
}:
assert lib.assertMsg (
  lib.isInt gestureRepeatThreshold && gestureRepeatThreshold >= 1 && gestureRepeatThreshold <= 32767
) "gestureRepeatThreshold must be an integer between 1 and 32767 raw-XY units.";
assert lib.assertMsg (
  lib.isInt gestureRepeatIntervalMs
  && gestureRepeatIntervalMs >= 1
  && gestureRepeatIntervalMs <= 60000
) "gestureRepeatIntervalMs must be an integer between 1 and 60000 milliseconds.";
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
      ./openlogi-reporting-recovery.patch
      ./openlogi-repeat-gestures.patch
      ./openlogi-managed-restart.patch
    ];
    postPatch =
      (old.postPatch or "")
      + ''
        # Both processes compile the same fixed-size pointer protocol.
        install -Dm644 ${../proto/src/pointer.rs} crates/openlogi-agent-core/src/remote/wire.rs
        substituteInPlace crates/openlogi-core/src/binding/swipe.rs \
          --replace-fail 'pub const REPEAT_THRESHOLD: i32 = 150;' \
                         'pub const REPEAT_THRESHOLD: i32 = ${toString gestureRepeatThreshold};' \
          --replace-fail 'pub const REPEAT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(300);' \
                         'pub const REPEAT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(${toString gestureRepeatIntervalMs});'
      ''
      + lib.optionalString stdenv.hostPlatform.isLinux ''
        install -Dm644 ${./tests/openlogi-capture.rs} crates/openlogi-hook/examples/hidpipe-capture.rs
        install -Dm644 ${./tests/openlogi-switch-recovery.rs} crates/openlogi-agent-core/tests/switch_recovery.rs
        install -Dm644 ${./tests/openlogi-repeat-gestures.rs} crates/openlogi-agent-core/tests/repeat_gestures.rs
      '';
    passthru = (old.passthru or { }) // {
      inherit withGui gestureRepeatThreshold gestureRepeatIntervalMs;
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
