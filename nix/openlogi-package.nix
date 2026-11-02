{ lib, openlogi }:
assert lib.assertMsg (
  openlogi.version == "0.6.25"
) "The hidpipe bridge patch targets OpenLogi 0.6.25.";
openlogi.overrideAttrs (old: {
  patches = (old.patches or [ ]) ++ [ ./openlogi-bridge.patch ];
  postPatch = (old.postPatch or "") + ''
    # Both processes compile the same fixed-size pointer protocol.
    install -Dm644 ${../proto/src/pointer.rs} crates/openlogi-agent-core/src/remote/wire.rs
  '';
})
