//! hidpipe-client: macOS 注入端。
//!
//! 连接 Linux 捕获端,把 HID 帧经 CGEventPost 注入系统。
//! 注入发生在 Quartz 层、位于 Karabiner 驱动层之下:本地改键规则
//! 不会作用于远端按键(不会双重映射),远端按键也不依赖本地规则
//! (不会时灵时不灵)。换键责任全部在捕获侧,恰好一次。
//!
//! 防卡键的三道防线:
//! 1. 会话建立时 reset_modifiers 抬一遍所有修饰键,治愈历史残留;
//! 2. 断线时 Injector Drop 释放本会话按住的键;
//! 3. SIGTERM/SIGINT 经共享的 PressedState 由信号线程兜底释放。

#![forbid(unsafe_code)]

mod table;

use std::collections::{HashSet, VecDeque};
use std::io;
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::{Duration, Instant};

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
/// HID page 0x07 的 CapsLock usage ID。
const HID_CAPS_LOCK: u16 = 0x39;
/// 每格滚轮折算的行数,对齐 macOS 原生鼠标的手感。
const LINES_PER_NOTCH: i32 = 3;
/// 返程触发:光标钳制在左缘时,时间窗内累计左推量超过阈值才交还控制。
/// 用滑动窗求和而非"反向即清零":高精度鼠标在边缘压住时
/// 会不断产出 ±1 的抖动,硬清零会让触发变成纯运气。
const RETURN_PUSH_THRESHOLD: i32 = 500;
const RETURN_PUSH_WINDOW: Duration = Duration::from_millis(1500);

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
    // 按住的键集合是可发送的纯数据,共享给信号线程兜底;
    // CGEventSource 不可发送,信号线程自建一个。
    let pressed = Arc::new(Mutex::new(PressedState::default()));
    {
        let pressed = Arc::clone(&pressed);
        ctrlc::set_handler(move || {
            let mut state = pressed.lock().unwrap();
            if let Ok(source) = CGEventSource::new(CGEventSourceStateID::CombinedSessionState) {
                release_all(&source, &mut state);
            }
            std::process::exit(0);
        })
        .expect("注册信号处理失败");
    }
    loop {
        match session(&args, Arc::clone(&pressed)) {
            Ok(()) => log!("对端关闭了连接"),
            Err(e) => log!("连接中断: {e}"),
        }
        sleep(RETRY_INTERVAL);
    }
}

fn session(args: &Args, pressed: Arc<Mutex<PressedState>>) -> io::Result<()> {
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

    let mut injector = Injector::new(args.flip_scroll, pressed);
    injector.reset_modifiers();
    loop {
        let frame = proto::read_frame(&mut stream)?;
        match frame {
            Frame::Ping => proto::write_frame(&mut stream, &Frame::Pong)?,
            frame => {
                if let Some(back) = injector.handle(frame) {
                    proto::write_frame(&mut stream, &back)?;
                }
            }
        }
    }
}

/// 当前按住的键与按钮。纯数据,可跨线程发送——这是信号处理器
/// 能兜底的结构前提。
#[derive(Default)]
struct PressedState {
    keys: HashSet<u16>,
    buttons: HashSet<MouseButton>,
}

struct Injector {
    source: CGEventSource,
    pressed: Arc<Mutex<PressedState>>,
    flip_scroll: bool,
    /// 光标钳制在左缘期间的左推量时间窗,(时刻, 位移)。
    edge_pushes: VecDeque<(Instant, i32)>,
}

impl Injector {
    fn new(flip_scroll: bool, pressed: Arc<Mutex<PressedState>>) -> Self {
        let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
            .expect("无法创建 CGEventSource——请在 系统设置 → 隐私与安全性 → 辅助功能 中授权本程序");
        Self {
            source,
            pressed,
            flip_scroll,
            edge_pushes: VecDeque::new(),
        }
    }

    /// 系统级残留的按下状态(上个进程被 kill、历史 bug 等)不会随
    /// 进程退出消失。每次建立会话时把所有修饰键抬一遍,治愈残留。
    /// 注意不能抬 CapsLock:macOS 对合成 CapsLock 的 up 也会翻转锁定态。
    fn reset_modifiers(&self) {
        const MODIFIERS: [CGKeyCode; 8] = [
            0x3B, 0x3E, // Control 左/右
            0x38, 0x3C, // Shift 左/右
            0x3A, 0x3D, // Option 左/右
            0x37, 0x36, // Command 左/右
        ];
        for code in MODIFIERS {
            post_key(&self.source, code, false, false);
        }
    }

    /// 处理一帧;若需要回话(如返程触发),返回要发回的帧。
    fn handle(&mut self, frame: Frame) -> Option<Frame> {
        match frame {
            Frame::Key { usage, state } => self.key(usage, state),
            Frame::Motion { dx, dy } => return self.motion(dx, dy),
            Frame::Button { button, state } => self.button(button, state),
            Frame::Wheel { dx, dy } => self.wheel(dx, dy),
            Frame::ReleaseAll => self.release_all(),
            _ => {}
        }
        None
    }

    fn key(&mut self, usage: u16, state: KeyState) {
        // macOS quirk:合成 CapsLock 事件在 down 和 up 时各翻转一次
        // 锁定状态。只在按下时投递,松开/重复吞掉;且不入 pressed_keys,
        // 否则 ReleaseAll 补发的 up 会在切回本地时多翻一次。
        if usage == HID_CAPS_LOCK {
            if state == KeyState::Press {
                if let Some(code) = table::hid_to_mac(usage) {
                    post_key(&self.source, code, true, false);
                }
            }
            return;
        }
        let Some(code) = table::hid_to_mac(usage) else {
            log!("未映射 HID usage,已忽略: {usage:#06x}");
            return;
        };
        post_key(
            &self.source,
            code,
            state != KeyState::Release,
            state == KeyState::Repeat,
        );
        let mut pressed = self.pressed.lock().unwrap();
        match state {
            KeyState::Press => {
                pressed.keys.insert(usage);
            }
            KeyState::Release => {
                pressed.keys.remove(&usage);
            }
            KeyState::Repeat => {}
        }
    }

    fn motion(&mut self, dx: i16, dy: i16) -> Option<Frame> {
        let mut p = current_location(&self.source)?;
        // v0 只钳制在主屏;多屏支持在 roadmap。
        let b = CGDisplay::main().bounds();
        p.x = (p.x + f64::from(dx)).clamp(b.origin.x, b.origin.x + b.size.width - 1.0);
        p.y = (p.y + f64::from(dy)).clamp(b.origin.y, b.origin.y + b.size.height - 1.0);

        // 按住按钮时的移动必须发拖拽事件,否则 macOS 上无法拖选/拖放。
        let held_button = self.pressed.lock().unwrap().buttons.iter().next().copied();
        let event_type = held_button.map_or(CGEventType::MouseMoved, |button| match button {
            MouseButton::Left => CGEventType::LeftMouseDragged,
            MouseButton::Right => CGEventType::RightMouseDragged,
            MouseButton::Middle => CGEventType::OtherMouseDragged,
        });
        let Ok(event) =
            CGEvent::new_mouse_event(self.source.clone(), event_type, p, CGMouseButton::Left)
        else {
            return None;
        };
        // 游戏等读相对位移的场景看 delta 字段,而非绝对位置。
        event.set_integer_value_field(EventField::MOUSE_EVENT_DELTA_X, i64::from(dx));
        event.set_integer_value_field(EventField::MOUSE_EVENT_DELTA_Y, i64::from(dy));
        event.post(CGEventTapLocation::HID);

        // 返程边缘检测:光标钳制在左缘、未按任何按钮时,
        // 左推量进时间窗;右移只是不进窗,不清零(容忍手抖)。
        // 光标一旦离开左缘(正常往里用),才清空窗口。
        let pinned = p.x <= b.origin.x && held_button.is_none();
        if pinned {
            if dx < 0 {
                self.edge_pushes.push_back((Instant::now(), i32::from(-dx)));
            }
            while self
                .edge_pushes
                .front()
                .is_some_and(|(t, _)| t.elapsed() > RETURN_PUSH_WINDOW)
            {
                self.edge_pushes.pop_front();
            }
            let pushed: i32 = self.edge_pushes.iter().map(|(_, v)| v).sum();
            if pushed > RETURN_PUSH_THRESHOLD {
                self.edge_pushes.clear();
                log!("光标推穿左缘,交还控制");
                return Some(Frame::ReturnToLocal);
            }
        } else {
            self.edge_pushes.clear();
        }
        None
    }

    fn button(&mut self, button: MouseButton, state: ButtonState) {
        post_button(&self.source, button, state);
        let mut pressed = self.pressed.lock().unwrap();
        match state {
            ButtonState::Press => {
                pressed.buttons.insert(button);
            }
            ButtonState::Release => {
                pressed.buttons.remove(&button);
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

    fn release_all(&mut self) {
        release_all(&self.source, &mut self.pressed.lock().unwrap());
    }
}

impl Drop for Injector {
    fn drop(&mut self) {
        self.release_all();
    }
}

/// 释放所有按住的键和按钮。自由函数形态,供正常路径(Injector)
/// 和信号路径(自建 source)共用。
fn release_all(source: &CGEventSource, pressed: &mut PressedState) {
    for usage in pressed.keys.drain() {
        if let Some(code) = table::hid_to_mac(usage) {
            post_key(source, code, false, false);
        }
    }
    let buttons: Vec<MouseButton> = pressed.buttons.drain().collect();
    for button in buttons {
        post_button(source, button, ButtonState::Release);
    }
}

fn post_key(source: &CGEventSource, code: CGKeyCode, down: bool, autorepeat: bool) {
    let Ok(event) = CGEvent::new_keyboard_event(source.clone(), code, down) else {
        log!("键盘事件创建失败: {code:#06x}");
        return;
    };
    if autorepeat {
        event.set_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT, 1);
    }
    event.post(CGEventTapLocation::HID);
}

fn post_button(source: &CGEventSource, button: MouseButton, state: ButtonState) {
    let Some(p) = current_location(source) else {
        return;
    };
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
    let Ok(event) = CGEvent::new_mouse_event(source.clone(), event_type, p, cg_button) else {
        return;
    };
    event.post(CGEventTapLocation::HID);
}

fn current_location(source: &CGEventSource) -> Option<CGPoint> {
    CGEvent::new(source.clone()).ok().map(|e| e.location())
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
