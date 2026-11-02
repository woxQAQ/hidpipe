# hidpipe

Cross-machine input pipe: capture on Linux, inject on macOS. Keyboard frames carry HID usage IDs. The optional OpenLogi backend forwards mouse input before bindings are resolved, so each machine can keep its own mouse configuration.

It fixes three structural problems of the deskflow/synergy family:

1. Fragile key remapping. Keysym translation plus cross-machine keymap negotiation are two state machines that drift apart.
2. Latency. The translation layer and event loop queuing add delay.
3. Weak failure recovery. No heartbeat-based self-healing, no safety valve against stuck keys.

## Architecture

```mermaid
flowchart LR
    subgraph Linux["Linux (server: machine with the keyboard and mouse)"]
        K[physical keyboard and mouse] --> KD[keyd: the only place remapping happens]
        KD -->|raw evdev events| S[hidpipe-server<br/>EVIOCGRAB + edge switching]
    end
    subgraph Wire["wire protocol: no semantics"]
        S -->|"HID usage ID frames<br/>plain TCP, TCP_NODELAY"| C
    end
    subgraph macOS["macOS (client)"]
        C[hidpipe-client<br/>CGEventPost @ HID tap] --> SYS[system]
        KB[Karabiner] -.->|driver level, cannot see injected events| SYS
    end
```

Three design invariants:

1. The protocol has no semantics. Only raw HID events go on the wire. No keymap negotiation, no modifier state machine, no cache. Nothing that can drift.
2. Keyboard remapping happens exactly once, at a fixed place. keyd on the capture side resolves stateful remaps like `overload(control, esc)` before keyboard events go on the wire. Quartz injection bypasses driver-level keyboard remapping. With the OpenLogi backend, mouse bindings are resolved exactly once on the machine currently being controlled.
3. Disconnect releases everything. When the connection dies, the server ungrabs at once and the client releases all held keys on Drop. Grab and key state live and die with the connection.

## Build

```bash
# Linux (capture side)
cargo build --release -p hidpipe-server

# macOS (injection side, build on the Mac)
cargo build --release -p hidpipe-client
```

`nix develop` provides a Rust toolchain. The flake also provides native packages
for `x86_64-linux`, `aarch64-linux`, and `aarch64-darwin`:

```bash
nix build                        # server on Linux, client on macOS
nix run . -- --help
nix flake check                  # build the native package and run protocol tests
```

## NixOS and Home Manager

Add hidpipe to your flake inputs:

```nix
inputs.hidpipe = {
  url = "github:woxQAQ/hidpipe";
  inputs.nixpkgs.follows = "nixpkgs";
};
```

On the Linux capture machine, import the NixOS module:

```nix
{ inputs, ... }:
{
  imports = [ inputs.hidpipe.nixosModules.default ];

  services.hidpipe = {
    enable = true;
    user = "alice"; # an existing desktop user
    keyboard = "keyd";
    mouse = "MX Master";
    openFirewall = true;
  };
}
```

The NixOS module adds the user to the `input` group and installs a systemd user
service, restricted to that user. Log out and back in after the first activation
so the user service manager picks up the new group membership. The service starts
with `graphical-session.target` and stops with the desktop session.

On macOS, import the Home Manager module:

```nix
{ inputs, ... }:
{
  imports = [ inputs.hidpipe.homeManagerModules.default ];

  services.hidpipe = {
    enable = true;
    host = "192.168.1.10";
    # flipScroll = true;
  };
}
```

This creates a launchd agent in the graphical login session. Grant
`hidpipe-client` itself access in System Settings > Privacy & Security >
Accessibility; the terminal's permission does not cover the agent. Use
Cmd+Shift+G in the file picker to select `~/.nix-profile/bin/hidpipe-client`
(or the `bin/hidpipe-client` under your Home Manager profile). A package upgrade
changes the executable's Nix store path and may require granting access again.
Logs go to `~/Library/Logs/hidpipe.log`. The client requests event-posting
permission before connecting and waits until access is granted. Revoking access
closes the connection on the next heartbeat, releasing Linux input.

Home Manager also supports the Linux server, using the same server options as
the NixOS module:

```nix
{ inputs, ... }:
{
  imports = [ inputs.hidpipe.homeManagerModules.default ];

  services.hidpipe = {
    enable = true;
    keyboard = "keyd";
    mouse = "MX Master";
  };
}
```

When using Home Manager on Linux, configure input permissions and the firewall
at the system level yourself. On NixOS, for example:

```nix
users.users.alice.extraGroups = [ "input" ];
networking.firewall.allowedTCPPorts = [ 24800 ];
```

Choose either NixOS or Home Manager to manage the Linux service. Both use the
unit name `hidpipe.service`. The compositor must import `WAYLAND_DISPLAY` into
the systemd user environment before starting the session target; NixOS Sway
integration and UWSM handle this. For a compositor with its own session target,
set `services.hidpipe.systemdTarget` accordingly. The server requires a Wayland
session and a compositor with layer-shell support.

| module option (`services.hidpipe.*`) | default | applies to |
|---|---|---|
| `enable` | `false` | both platforms |
| `package` | native hidpipe package | both platforms |
| `port` | `24800` | both platforms |
| `user` | required when enabled | NixOS only |
| `openFirewall` | `false` | NixOS only |
| `bind` | `"0.0.0.0"` | Linux |
| `keyboard` / `mouse` | `null` (auto-detect) | Linux |
| `swapCapsEsc` | `false` | Linux |
| `systemdTarget` | `"graphical-session.target"` | Linux |
| `host` | required when enabled | macOS, Home Manager |
| `flipScroll` | `false` | macOS, Home Manager |

Check the Linux service with `systemctl --user status hidpipe` and
`journalctl --user -u hidpipe -f`. On macOS, use
`launchctl print gui/$(id -u)/org.nix-community.home.hidpipe`.

## OpenLogi mouse backend

Enable this backend on **both** peers to use macOS's own OpenLogi bindings instead
of forwarding Linux-generated shortcuts. The bridge patch targets OpenLogi
**0.6.25**; `nix/openlogi-package.nix` applies it without changing dependencies.
The protocol module in `proto/src/pointer.rs` is compiled into both programs.
The editable OpenLogi worktree can link that file at
`crates/openlogi-agent-core/src/remote/wire.rs`.

On NixOS:

```nix
services.hidpipe = {
  enable = true;
  user = "alice";
  openFirewall = true;
  openlogi = {
    enable = true;
    mouse = "MX Master 2S";
    gui.enable = true; # default; retain the OpenLogi settings UI
    # device = "<OpenLogi config key>"; # required for multiple configured mice
  };
};
```

Disable logiops and other services that capture or divert the same mouse. The
module provides the patched OpenLogi GUI and agent, grants the user `input` and
`uinput` membership, and enables `/dev/uinput`. Log out and back in after changing
groups. OpenLogi owns the physical mouse for the entire session; hidpipe only grabs
the keyd keyboard when entering remote mode. If keyd's wildcard identifies the
mouse as a keyboard, exclude its vendor/product ID. For the MX Master 2S:

```nix
services.keyd.keyboards.default.ids = [ "*" "-046d:4069" ];
```

OpenLogi preserves that ID on its virtual mouse, so the exclusion covers both.
The NixOS module also reapplies udev permissions to already-connected Logitech
HID devices when its rules change.

`services.hidpipe.openlogi.gui.enable` defaults to `true`. This builds and installs
the Linux desktop launcher or macOS application bundle alongside the agent, and
leaves GUI startup to the user. Set it to `false` for a CLI and agent installation.
The package function exposes the same choice as `withGui`:

```nix
inputs.hidpipe.packages.${pkgs.stdenv.hostPlatform.system}.openlogi.override {
  withGui = false;
}
```

On macOS with Home Manager:

```nix
services.hidpipe = {
  enable = true;
  host = "linux-host.local";
  openlogi.enable = true;
  # openlogi.device = "<Mac OpenLogi config key>";
};
```

Use the OpenLogi GUI on each machine to configure that machine's actions. The Mac
agent uses its saved device selection unless `openlogi.device` is explicit; the
mouse need not be physically connected to the Mac. Linux automatically chooses
its first online pointing device unless a key is explicit. With multiple mice,
set the key explicitly on both peers. Device keys and bindings do not have to
match across systems. Quit any previously running OpenLogi agent once when
migrating to the managed service, and remove its old login item. Grant the patched
`openlogi-agent` and `hidpipe-client` Accessibility access on macOS.

The module manages `openlogi-agent.service` on Linux and a launchd agent on macOS.
The bridged package owns the capture-readiness, virtual-device identity and
mutable-config fixes; consumers do not need to apply additional OpenLogi patches.
Logs on the Mac are in `~/Library/Logs/openlogi-agent.log`. Socket paths default to
`~/.cache/openlogi-hidpipe/pointer.sock` and must be in a private directory (0700).
The agent uses `OPENLOGI_MANAGED_SERVICE=1` to leave autostart management to Nix.
Override `openlogi.basePackage` when using another nixpkgs input:

```nix
services.hidpipe.openlogi.basePackage = fastest-pkg.openlogi;
```

For manual operation, give the agent `OPENLOGI_BRIDGE_SOCKET` and optionally
`OPENLOGI_BRIDGE_DEVICE`; on Linux also give it `OPENLOGI_BRIDGE_MOUSE` (a name
substring selecting exactly one physical Logitech mouse). Pass
`--openlogi-socket <path>` to hidpipe on each machine. The agent's socket is
local-only and versioned independently of GUI IPC.

The main wheel uses ordinary kernel ticks by default, reducing accidental
scrolling from sub-notch motion on both Linux and the remote Mac. The thumb
wheel keeps its existing resolution. To retain smooth high-resolution main-wheel
input, set this option on the Linux capture machine:

```nix
services.hidpipe.openlogi.highResolutionScroll = true;
```

For manual operation, set `OPENLOGI_BRIDGE_WHEEL_HIRES=1` on the Linux agent.
The wheel policy takes effect when that agent starts.

Capture is a connection-scoped lease with a 750 ms heartbeat deadline. A held
mouse button prevents entry. Disconnect, a stalled server, input-hook failure,
or a full capture queue revokes the lease and restores Linux routing. Buttons
still held at revocation are quarantined until release; pending gestures are
cancelled without firing a click. On macOS, binding lookup happens before normal
mouse injection, and a held button keeps its original disposition across config
or foreground-app changes. Sub-notch main-wheel motion is retained when
high-resolution scrolling is enabled. Hardware settings
such as DPI and SmartShift belong to the Linux profile; this bridge does not
forward hardware actions from the Mac back to the physical device.

The network protocol is version 1. Upgrade both hidpipe peers together; a
version or pointer-backend mismatch is rejected before capture. Native evdev
capture remains available when OpenLogi is disabled.

## Run

Linux needs a Wayland session with layer-shell support and read access to
`/dev/input/event*`; membership in the `input` group is enough for device access.

```bash
hidpipe-server                    # auto-detect devices, prefer the keyd virtual keyboard
hidpipe-server --keyboard "keyd" --mouse "MX Master" --port 24800
```

macOS:

```bash
hidpipe-client <linux-ip>         # retries once a second until connected
```

On first run, grant Accessibility access to the executable. A launchd service needs its own permission; the terminal's permission does not cover it. Until event-posting access is granted, the client waits without connecting to Linux.

Switching works like this. Push the cursor past the right edge of the Linux screen to enter remote mode; the trigger is a layer-shell barrier, so the compositor itself detects the crossing. On the Mac, push the cursor against the left edge and keep pushing to hand control back. In remote mode the Linux cursor freezes because the devices are grabbed, and all input goes to the Mac.

## Options

| server | default | description |
|---|---|---|
| `--bind` / `--port` | 0.0.0.0 / 24800 | listen address and port |
| `--keyboard` / `--mouse` | auto-detect | device name substring filter |
| `--swap-caps-esc` | off | redo the caps/esc swap on the capture side (only when keyd does not do it) |

| client | default | description |
|---|---|---|
| `--port` | 24800 | server port |
| `--flip-scroll` | off | reverse scroll direction |

## Known limitations (v0, weekend prototype)

- Plain TCP, trusted LANs only. TLS is on the roadmap.
- Media keys (volume, playback) are not mapped. They live on the HID consumer page, not the keyboard page.
- macOS SecureEventInput: when a password field has focus, the system discards all synthetic events. This is platform policy; no injection-based tool, deskflow included, can bypass it.
- The mouse is clamped to the main display. Multi-monitor support is on the roadmap.
- The barrier sits on the right edge of every output. With multiple monitors, the seam between two outputs triggers it too. Per-output configuration is on the roadmap.
- The return direction (Mac to Linux) only watches the left edge of the Mac's main display.
- Keyboard device loss (including a keyd restart) releases remote input and exits. The systemd service restarts the server and opens the current device. A directly captured evdev mouse requires a server restart after hot-plug.
- Side mouse buttons (back, forward) are forwarded with the OpenLogi backend; native evdev capture still forwards three buttons.

## Roadmap

- [ ] TLS (self-signed certificate pair, fingerprint authentication)
- [x] nix modules: systemd user service on Linux, launchd agent on macOS
- [ ] multi-monitor: barrier configurable per output, cursor clamping across Mac displays
- [ ] map the Mac cursor's initial Y by screen ratio when entering remote mode
- [ ] media keys (consumer page)
- [ ] clipboard sync (needs careful design, may stay out)

## Why edge switching came back

The first draft rejected pseudo edge detection that accumulates deltas to estimate the cursor position, because it always drifts. The current implementation uses two deterministic paths instead: a layer-shell barrier on the way in (the compositor itself sends the enter event), and precise cursor clamping on the client for the way back. There is no estimation anywhere.
