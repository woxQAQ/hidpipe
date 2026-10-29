//! Command line arguments. Hand-rolled with zero dependencies; the flag
//! surface is too small to justify clap.

use evdev::Key;

pub struct Args {
    pub bind: String,
    pub port: u16,
    /// Device name substring filter; None means auto-detect.
    pub keyboard: Option<String>,
    pub mouse: Option<String>,
    pub switch_key: Key,
    /// Key probe mode: print the name of every pressed key without
    /// switching or forwarding. Used to pick a --switch-key that physically
    /// exists and is rarely used.
    pub probe: bool,
    /// Disable the Wayland edge barrier (for non-Wayland sessions or
    /// hotkey-only use).
    pub no_barrier: bool,
    /// Swap caps and esc on the capture side; only affects keys sent to the
    /// remote. When the swap lives in the xkb layer (above evdev), the
    /// remote side never sees it, so this flag redoes it. Do not enable it
    /// if keyd or udev already does the swap.
    pub swap_caps_esc: bool,
}

const USAGE: &str = "\
hidpipe-server: Linux input capture side

Usage: hidpipe-server [options]

Options:
  --bind <addr>        listen address (default 0.0.0.0)
  --port <port>        listen port (default 24800)
  --keyboard <substr>  match keyboard device name (default auto-detect, prefers the keyd virtual keyboard)
  --mouse <substr>     match mouse device name (default auto-detect)
  --switch-key <name>  local/remote switch key (default scrolllock, names below)
  --probe              key probe mode: print what you press, for picking a switch key
  --no-barrier         disable the right-edge barrier, hotkey only
  --swap-caps-esc      swap caps and esc for keys sent to the remote
  -h, --help           show this help

Needs read access to /dev/input/event* (input group or root).

--switch-key accepts:
  scrolllock | pause | printscreen | menu | compose
  insert | delete | home | end | pageup | pagedown
  kpenter | kpplus | kpminus | f1..f24
";

impl Args {
    pub fn parse() -> Self {
        let mut args = Self {
            bind: "0.0.0.0".into(),
            port: 24800,
            keyboard: None,
            mouse: None,
            switch_key: Key::KEY_SCROLLLOCK,
            probe: false,
            no_barrier: false,
            swap_caps_esc: false,
        };
        let mut it = std::env::args().skip(1);
        while let Some(arg) = it.next() {
            let mut take = |flag: &str| -> String {
                it.next().unwrap_or_else(|| {
                    eprintln!("{flag} requires a value\n{USAGE}");
                    std::process::exit(2);
                })
            };
            match arg.as_str() {
                "--bind" => args.bind = take("--bind"),
                "--port" => {
                    args.port = take("--port").parse().unwrap_or_else(|_| {
                        eprintln!("--port is not a valid port number\n{USAGE}");
                        std::process::exit(2);
                    })
                }
                "--keyboard" => args.keyboard = Some(take("--keyboard")),
                "--mouse" => args.mouse = Some(take("--mouse")),
                "--switch-key" => args.switch_key = parse_switch_key(&take("--switch-key")),
                "--probe" => args.probe = true,
                "--no-barrier" => args.no_barrier = true,
                "--swap-caps-esc" => args.swap_caps_esc = true,
                "-h" | "--help" => {
                    print!("{USAGE}");
                    std::process::exit(0);
                }
                other => {
                    eprintln!("unknown argument {other}\n{USAGE}");
                    std::process::exit(2);
                }
            }
        }
        args
    }
}

fn parse_switch_key(name: &str) -> Key {
    let name = name.to_ascii_lowercase();
    let fixed = match name.as_str() {
        "scrolllock" => Some(Key::KEY_SCROLLLOCK),
        "pause" => Some(Key::KEY_PAUSE),
        "printscreen" => Some(Key::KEY_SYSRQ),
        "menu" => Some(Key::KEY_MENU),
        "compose" => Some(Key::KEY_COMPOSE),
        "insert" => Some(Key::KEY_INSERT),
        "delete" => Some(Key::KEY_DELETE),
        "home" => Some(Key::KEY_HOME),
        "end" => Some(Key::KEY_END),
        "pageup" => Some(Key::KEY_PAGEUP),
        "pagedown" => Some(Key::KEY_PAGEDOWN),
        "kpenter" => Some(Key::KEY_KPENTER),
        "kpplus" => Some(Key::KEY_KPPLUS),
        "kpminus" => Some(Key::KEY_KPMINUS),
        _ => None,
    };
    if let Some(key) = fixed {
        return key;
    }
    // F1..F12 encode as 59..=68 in evdev, F13..F24 as 183..=194.
    if let Some(n) = name.strip_prefix('f').and_then(|s| s.parse::<u16>().ok()) {
        return match n {
            1..=12 => Key::new(58 + n),
            13..=24 => Key::new(170 + n),
            _ => unknown_key(&name),
        };
    }
    unknown_key(&name)
}

fn unknown_key(name: &str) -> ! {
    eprintln!("unsupported switch key {name}, see --help for names");
    std::process::exit(2);
}
