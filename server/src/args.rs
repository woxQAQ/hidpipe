//! 命令行参数。零依赖手撸——参数面就这几个,不值得上 clap。

use evdev::Key;

pub struct Args {
    pub bind: String,
    pub port: u16,
    /// 设备名子串过滤器,None 表示自动探测。
    pub keyboard: Option<String>,
    pub mouse: Option<String>,
    pub switch_key: Key,
    /// 探键模式:打印每个按下的键名,不执行切换/转发。
    /// 用于给 --switch-key 选一个物理上存在且不常用的键。
    pub probe: bool,
    /// 禁用 Wayland 边缘屏障(非 Wayland 会话或只要热键时用)。
    pub no_barrier: bool,
    /// 在捕获侧做 caps ↔ esc 互换,只影响发往远端的按键。
    /// 当本地会话的互换做在 xkb 层(evdev 之上)时,远端拿不到互换结果,
    /// 用这个开关补上;若互换已在 keyd/udev 层完成则不要开。
    pub swap_caps_esc: bool,
}

const USAGE: &str = "\
hidpipe-server — Linux 输入捕获端

用法: hidpipe-server [选项]

选项:
  --bind <地址>        监听地址 (默认 0.0.0.0)
  --port <端口>        监听端口 (默认 24800)
  --keyboard <子串>    键盘设备名匹配 (默认自动探测,优先 keyd 虚拟键盘)
  --mouse <子串>       鼠标设备名匹配 (默认自动探测)
  --switch-key <键名>  本地/远程切换键 (默认 scrolllock,可选名见下)
  --probe              探键模式:按什么打印什么,用于选切换键
  --no-barrier         禁用屏幕右缘屏障,只用热键切换
  --swap-caps-esc      发往远端的按键做 caps ↔ esc 互换
  -h, --help           显示本帮助

需要读 /dev/input/event* 的权限(input 组或 root)。

--switch-key 可选键名:
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
                    eprintln!("{flag} 缺少参数值\n{USAGE}");
                    std::process::exit(2);
                })
            };
            match arg.as_str() {
                "--bind" => args.bind = take("--bind"),
                "--port" => {
                    args.port = take("--port").parse().unwrap_or_else(|_| {
                        eprintln!("--port 不是合法端口号\n{USAGE}");
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
                    eprintln!("未知参数 {other}\n{USAGE}");
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
    // F1..F12 的 evdev 编码是 59..=68,F13..F24 是 183..=194。
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
    eprintln!("不支持的切换键 {name},可选键名见 --help");
    std::process::exit(2);
}
