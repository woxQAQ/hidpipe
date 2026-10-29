//! hidpipe-client: macOS 注入端。
//!
//! 连接 Linux 捕获端,把 HID 帧经 CGEventPost 注入系统。
//! 注入发生在 Quartz 层、位于 Karabiner 驱动层之下:本地改键规则
//! 不会作用于远端按键(不会双重映射),远端按键也不依赖本地规则
//! (不会时灵时不灵)。换键责任全部在捕获侧,恰好一次。
//!
//! 断线时 Drop 自动释放所有按住的键与按钮——对端不会残留卡住的修饰键。

#![forbid(unsafe_code)]

mod table;

use std::collections::HashSet;
use std::io;
use std::net::TcpStream;
use std::thread::sleep;
use std::time::Duration;

use core_graphics::display::CGDisplay;
use core_graphics::event::{
    CGEvent, CGEventTapLocation, CGEventType, CGKeyCode, CGMouseButton, EventField, ScrollEventUnit,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;

use hidpipe_proto as proto;
use proto::{ButtonState, Frame, KeyState, MouseButton};

const READ_TIMEOUT: Duration = Duration::from_secs(6);
const RETRY_INTERVAL: Duration = Duration::from_secs(1);
/// 每格滚轮折算的行数,对齐 macOS 原生鼠标的手感。
const LINES_PER_NOTCH: i32 = 3;

/// 运行日志统一带本地时间戳,格式 [HH:MM:SS]。
macro_rules! log {
    ($($arg:tt)*) => {
        eprintln!("[{}] {}", timestamp(), format_args!($($arg)*))
    };
}

fn timestamp() -> String {
    let fmt = time::macros::format_description!("[hour]:[minute]:[second]");
    time::OffsetDateTime::now_local()
        .unwrap_or_else(|_| time::OffsetDateTime::now_utc())
        .format(&fmt)
        .unwrap_or_default()
}

fn main() {
    let args = Args::parse();
    loop {
        match session(&args) {
            Ok(()) => log!("对端关闭了连接"),
            Err(e) => log!("连接中断: {e}"),
        }
        sleep(RETRY_INTERVAL);
    }
}

fn session(args: &Args) -> io::Result<()> {
    let mut stream = TcpStream::connect((args.host.as_str(), args.port))?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    proto::write_frame(
        &mut stream,
        &Frame::Hello {
            version: proto::VERSION,
        },
    )?;
    log!("已连接 {}:{}", args.host, args.port);

    let mut injector = Injector::new(args.flip_scroll);
    loop {
        let frame = proto::read_frame(&mut stream)?;
        match frame {
            Frame::Ping => proto::write_frame(&mut stream, &Frame::Pong)?,
            frame => injector.handle(frame),
        }
    }
}

struct Injector {
    source: CGEventSource,
    pressed_keys: HashSet<u16>,
    pressed_buttons: HashSet<MouseButton>,
    flip_scroll: bool,
}

impl Injector {
    fn new(flip_scroll: bool) -> Self {
        let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
            .expect("无法创建 CGEventSource——请在 系统设置 → 隐私与安全性 → 辅助功能 中授权本程序");
        Self {
            source,
            pressed_keys: HashSet::new(),
            pressed_buttons: HashSet::new(),
            flip_scroll,
        }
    }

    fn handle(&mut self, frame: Frame) {
        match frame {
            Frame::Key { usage, state } => self.key(usage, state),
            Frame::Motion { dx, dy } => self.motion(dx, dy),
            Frame::Button { button, state } => self.button(button, state),
            Frame::Wheel { dx, dy } => self.wheel(dx, dy),
            Frame::ReleaseAll => self.release_all(),
            _ => {}
        }
    }

    fn key(&mut self, usage: u16, state: KeyState) {
        let Some(code) = table::hid_to_mac(usage) else {
            log!("未映射 HID usage,已忽略: {usage:#06x}");
            return;
        };
        self.post_key(code, state != KeyState::Release, state == KeyState::Repeat);
        match state {
            KeyState::Press => {
                self.pressed_keys.insert(usage);
            }
            KeyState::Release => {
                self.pressed_keys.remove(&usage);
            }
            KeyState::Repeat => {}
        }
    }

    fn post_key(&self, code: CGKeyCode, down: bool, autorepeat: bool) {
        let Ok(event) = CGEvent::new_keyboard_event(self.source.clone(), code, down) else {
            log!("键盘事件创建失败: {code:#06x}");
            return;
        };
        if autorepeat {
            event.set_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT, 1);
        }
        event.post(CGEventTapLocation::HID);
    }

    fn motion(&mut self, dx: i16, dy: i16) {
        let Some(mut p) = self.location() else { return };
        // v0 只钳制在主屏;多屏支持在 roadmap。
        let b = CGDisplay::main().bounds();
        p.x = (p.x + f64::from(dx)).clamp(b.origin.x, b.origin.x + b.size.width - 1.0);
        p.y = (p.y + f64::from(dy)).clamp(b.origin.y, b.origin.y + b.size.height - 1.0);

        // 按住按钮时的移动必须发拖拽事件,否则 macOS 上无法拖选/拖放。
        let event_type = self.pressed_buttons.iter().next().map_or(
            CGEventType::MouseMoved,
            |button| match button {
                MouseButton::Left => CGEventType::LeftMouseDragged,
                MouseButton::Right => CGEventType::RightMouseDragged,
                MouseButton::Middle => CGEventType::OtherMouseDragged,
            },
        );
        let Ok(event) =
            CGEvent::new_mouse_event(self.source.clone(), event_type, p, CGMouseButton::Left)
        else {
            return;
        };
        // 游戏等读相对位移的场景看 delta 字段,而非绝对位置。
        event.set_integer_value_field(EventField::MOUSE_EVENT_DELTA_X, i64::from(dx));
        event.set_integer_value_field(EventField::MOUSE_EVENT_DELTA_Y, i64::from(dy));
        event.post(CGEventTapLocation::HID);
    }

    fn button(&mut self, button: MouseButton, state: ButtonState) {
        let Some(p) = self.location() else { return };
        let (event_type, cg_button) = match (button, state) {
            (MouseButton::Left, ButtonState::Press) => {
                (CGEventType::LeftMouseDown, CGMouseButton::Left)
            }
            (MouseButton::Left, ButtonState::Release) => {
                (CGEventType::LeftMouseUp, CGMouseButton::Left)
            }
            (MouseButton::Right, ButtonState::Press) => {
                (CGEventType::RightMouseDown, CGMouseButton::Right)
            }
            (MouseButton::Right, ButtonState::Release) => {
                (CGEventType::RightMouseUp, CGMouseButton::Right)
            }
            (MouseButton::Middle, ButtonState::Press) => {
                (CGEventType::OtherMouseDown, CGMouseButton::Center)
            }
            (MouseButton::Middle, ButtonState::Release) => {
                (CGEventType::OtherMouseUp, CGMouseButton::Center)
            }
        };
        let Ok(event) = CGEvent::new_mouse_event(self.source.clone(), event_type, p, cg_button)
        else {
            return;
        };
        event.post(CGEventTapLocation::HID);
        match state {
            ButtonState::Press => {
                self.pressed_buttons.insert(button);
            }
            ButtonState::Release => {
                self.pressed_buttons.remove(&button);
            }
        }
    }

    fn wheel(&self, dx: i8, dy: i8) {
        let sign = if self.flip_scroll { -1 } else { 1 };
        // 与 CGEventCreateScrollWheelEvent 约定一致:
        // 第 1 个轮是纵轴,wheel_count=1;横轴滚动时 wheel_count=2,值放第 2 位。
        if dy != 0 {
            self.scroll(1, i32::from(dy) * LINES_PER_NOTCH * sign, 0, 0);
        }
        if dx != 0 {
            self.scroll(2, 0, i32::from(dx) * LINES_PER_NOTCH * sign, 0);
        }
    }

    fn scroll(&self, wheel_count: u32, w1: i32, w2: i32, w3: i32) {
        let Ok(event) = CGEvent::new_scroll_event(
            self.source.clone(),
            ScrollEventUnit::LINE,
            wheel_count,
            w1,
            w2,
            w3,
        ) else {
            return;
        };
        event.post(CGEventTapLocation::HID);
    }

    /// 释放所有按住的键和按钮。切回本地/断线前由对端发 ReleaseAll 触发,
    /// 进程退出时由 Drop 兜底。
    fn release_all(&mut self) {
        let keys: Vec<u16> = self.pressed_keys.drain().collect();
        for usage in keys {
            if let Some(code) = table::hid_to_mac(usage) {
                self.post_key(code, false, false);
            }
        }
        let buttons: Vec<MouseButton> = self.pressed_buttons.drain().collect();
        for button in buttons {
            self.button(button, ButtonState::Release);
        }
    }

    fn location(&self) -> Option<CGPoint> {
        CGEvent::new(self.source.clone()).ok().map(|e| e.location())
    }
}

impl Drop for Injector {
    fn drop(&mut self) {
        self.release_all();
    }
}

struct Args {
    host: String,
    port: u16,
    /// 反转滚轮方向。macOS 的"自然滚动"设置不影响合成事件,
    /// 若手感反了加这个开关。
    flip_scroll: bool,
}

const USAGE: &str = "\
hidpipe-client — macOS 输入注入端

用法: hidpipe-client <服务器地址> [选项]

选项:
  --port <端口>   服务器端口 (默认 24800)
  --flip-scroll   反转滚轮方向
  -h, --help      显示本帮助

首次运行需在 系统设置 → 隐私与安全性 → 辅助功能 中授权运行它的终端。
";

impl Args {
    fn parse() -> Self {
        let mut args = Self {
            host: String::new(),
            port: 24800,
            flip_scroll: false,
        };
        let mut it = std::env::args().skip(1);
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--port" => {
                    let value = it.next().unwrap_or_else(|| {
                        eprintln!("--port 缺少参数值\n{USAGE}");
                        std::process::exit(2);
                    });
                    args.port = value.parse().unwrap_or_else(|_| {
                        eprintln!("--port 不是合法端口号\n{USAGE}");
                        std::process::exit(2);
                    });
                }
                "--flip-scroll" => args.flip_scroll = true,
                "-h" | "--help" => {
                    print!("{USAGE}");
                    std::process::exit(0);
                }
                other if other.starts_with('-') => {
                    eprintln!("未知参数 {other}\n{USAGE}");
                    std::process::exit(2);
                }
                other => args.host = other.to_string(),
            }
        }
        if args.host.is_empty() {
            eprintln!("缺少服务器地址\n{USAGE}");
            std::process::exit(2);
        }
        args
    }
}
