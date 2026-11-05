//! Command line arguments. Hand-rolled with zero dependencies; the flag
//! surface is too small to justify clap.

pub struct Args {
    pub bind: String,
    pub port: u16,
    /// Device name substring filter; None means auto-detect.
    pub keyboard: Option<String>,
    pub mouse: Option<String>,
    pub openlogi_socket: Option<std::path::PathBuf>,
    /// Swap caps and esc on the capture side; only affects keys sent to the
    /// remote. When the swap lives in the xkb layer (above evdev), the
    /// remote side never sees it, so this flag redoes it. Do not enable it
    /// if keyd or udev already does the swap.
    pub swap_caps_esc: bool,
    pub latency_stats: bool,
    pub clipboard: bool,
    pub clipboard_port: u16,
}

const USAGE: &str = "\
hidpipe-server: Linux input capture side

Usage: hidpipe-server [options]

Options:
  --bind <addr>        listen address (default 0.0.0.0)
  --port <port>        listen port (default 24800)
  --keyboard <substr>  match keyboard device name (default auto-detect, prefers the keyd virtual keyboard)
  --mouse <substr>     match mouse device name (default auto-detect)
  --openlogi-socket <path>  capture the mouse through an OpenLogi bridge
  --swap-caps-esc      swap caps and esc for keys sent to the remote
  --clipboard         share text clipboard over a separate trusted-LAN connection
  --clipboard-port <port>  clipboard port (default 24801; requires --clipboard)
  --latency-stats      log local timing summaries every 5 seconds
  -h, --help           show this help

Needs read access to /dev/input/event* (input group or root).
Requires a Wayland session with layer-shell support.
";

impl Args {
    pub fn parse() -> Self {
        let mut args = Self {
            bind: "0.0.0.0".into(),
            port: 24800,
            keyboard: None,
            mouse: None,
            openlogi_socket: None,
            swap_caps_esc: false,
            latency_stats: false,
            clipboard: false,
            clipboard_port: 24801,
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
                "--openlogi-socket" => {
                    args.openlogi_socket = Some(take("--openlogi-socket").into())
                }
                "--clipboard" => args.clipboard = true,
                "--clipboard-port" => {
                    args.clipboard_port = take("--clipboard-port").parse().unwrap_or_else(|_| {
                        eprintln!("--clipboard-port is not a valid port number\n{USAGE}");
                        std::process::exit(2);
                    });
                }
                "--latency-stats" => args.latency_stats = true,
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
        if args.clipboard && args.clipboard_port == args.port {
            eprintln!("clipboard and input ports must differ");
            std::process::exit(2);
        }
        if args.openlogi_socket.is_some() && args.mouse.is_some() {
            eprintln!("--mouse and --openlogi-socket are mutually exclusive");
            std::process::exit(2);
        }
        args
    }
}
