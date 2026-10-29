//! hidpipe-server: Linux 捕获端。
//!
//! 职责只有三件:
//! 1. 从 evdev 读原始输入(键盘优先选 keyd 虚拟设备,即本地改键之后的确定来源);
//! 2. 热键切换本地/远程模式。远程模式下 EVIOCGRAB 独占键鼠,
//!    事件翻译成 HID usage ID 帧发给 macOS 注入端;
//! 3. 断线即 ungrab——grab 状态随连接死亡释放,绝不残留。

#![forbid(unsafe_code)]

mod args;
mod hid;
mod net;

use std::io;
use std::net::TcpListener;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread::sleep;
use std::time::Duration;

use evdev::{Device, InputEventKind, Key, RelativeAxisType};
use hidpipe_proto::{ButtonState, Frame, KeyState, MouseButton};

use args::Args;
use net::{Connection, NetMsg};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Local,
    Remote,
}

#[derive(Clone, Copy)]
enum DeviceKind {
    Keyboard,
    Mouse,
}

impl DeviceKind {
    fn name(self) -> &'static str {
        match self {
            DeviceKind::Keyboard => "键盘",
            DeviceKind::Mouse => "鼠标",
        }
    }
}

fn main() -> io::Result<()> {
    let args = Args::parse();
    let (kbd, kbd_path) = open_device(DeviceKind::Keyboard, args.keyboard.as_deref())?;
    let (mouse, mouse_path) = open_device(DeviceKind::Mouse, args.mouse.as_deref())?;
    eprintln!("键盘: {} ({})", kbd.name().unwrap_or("?"), kbd_path.display());
    eprintln!("鼠标: {} ({})", mouse.name().unwrap_or("?"), mouse_path.display());
    eprintln!("切换键: {:?} | 端口: {}", args.switch_key, args.port);
    set_nonblocking(&kbd)?;
    set_nonblocking(&mouse)?;

    let listener = TcpListener::bind((args.bind.as_str(), args.port))?;
    listener.set_nonblocking(true)?;

    let (net_tx, net_rx) = mpsc::channel::<NetMsg>();
    let mut app = App {
        kbd,
        mouse,
        conn: None,
        conn_id: 0,
        mode: Mode::Local,
        motion: (0, 0),
        wheel: (0, 0),
        switch_key: args.switch_key,
        swap_caps_esc: args.swap_caps_esc,
        listener,
        net_tx,
        net_rx,
    };

    loop {
        app.accept_pending();
        app.handle_net_msgs();
        app.drain_keyboard();
        app.drain_mouse();
        sleep(Duration::from_millis(1));
    }
}

struct App {
    kbd: Device,
    mouse: Device,
    conn: Option<Connection>,
    conn_id: u64,
    mode: Mode,
    /// SYN_REPORT 之间累加的相对量。
    motion: (i64, i64),
    wheel: (i64, i64),
    switch_key: Key,
    swap_caps_esc: bool,
    listener: TcpListener,
    net_tx: mpsc::Sender<NetMsg>,
    net_rx: mpsc::Receiver<NetMsg>,
}

impl App {
    /// 新连接直接顶掉旧连接;旧线程会自行退出,其迟到的 Dead 携带旧代数,
    /// 在 handle_net_msgs 里被忽略。
    fn accept_pending(&mut self) {
        while let Ok((stream, addr)) = self.listener.accept() {
            eprintln!("客户端接入: {addr}");
            self.conn_id += 1;
            match Connection::spawn(stream, self.net_tx.clone(), self.conn_id) {
                Ok(conn) => self.conn = Some(conn),
                Err(e) => eprintln!("连接初始化失败: {e}"),
            }
        }
    }

    fn handle_net_msgs(&mut self) {
        while let Ok(NetMsg::Dead(id)) = self.net_rx.try_recv() {
            if self.conn.as_ref().is_some_and(|c| c.id == id) {
                eprintln!("连接死亡,回到本地模式");
                self.conn = None;
                self.leave_remote();
            }
        }
    }

    fn drain_keyboard(&mut self) {
        let events = match fetch_all(&mut self.kbd) {
            Ok(events) => events,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
            Err(e) => {
                // 设备被拔掉等硬错误:回本地模式等人工处理,好过静默丢输入。
                eprintln!("键盘读取失败({e}),回到本地模式");
                self.leave_remote();
                return;
            }
        };
        for event in events {
            if let InputEventKind::Key(key) = event.kind() {
                self.on_key(key, event.value());
            }
        }
    }

    fn drain_mouse(&mut self) {
        let events = match fetch_all(&mut self.mouse) {
            Ok(events) => events,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
            Err(e) => {
                eprintln!("鼠标读取失败({e}),回到本地模式");
                self.leave_remote();
                return;
            }
        };
        for event in events {
            match event.kind() {
                InputEventKind::Key(key) => self.on_mouse_button(key, event.value()),
                InputEventKind::RelAxis(axis) => self.on_rel(axis, event.value()),
                InputEventKind::Synchronization(_) => self.flush_motion(),
                _ => {}
            }
        }
    }

    /// value: 0 = 松开,1 = 按下,2 = 长按重复。
    fn on_key(&mut self, key: Key, value: i32) {
        if key == self.switch_key {
            if value == 1 {
                self.toggle_mode();
            }
            // 切换键本身不透传——它是本机的控制面,不属于数据面。
            return;
        }
        if self.mode != Mode::Remote {
            return;
        }
        let key = match (self.swap_caps_esc, key) {
            (true, Key::KEY_CAPSLOCK) => Key::KEY_ESC,
            (true, Key::KEY_ESC) => Key::KEY_CAPSLOCK,
            (_, key) => key,
        };
        let Some(usage) = hid::evdev_to_hid(key) else {
            if value == 1 {
                eprintln!("未映射按键,已忽略: {key:?}");
            }
            return;
        };
        let state = match value {
            0 => KeyState::Release,
            1 => KeyState::Press,
            _ => KeyState::Repeat,
        };
        self.send(Frame::Key { usage, state });
    }

    fn on_mouse_button(&mut self, key: Key, value: i32) {
        if self.mode != Mode::Remote || value == 2 {
            return;
        }
        let button = match key {
            Key::BTN_LEFT => MouseButton::Left,
            Key::BTN_RIGHT => MouseButton::Right,
            Key::BTN_MIDDLE => MouseButton::Middle,
            // 侧键等 v0 不转发。
            _ => return,
        };
        let state = if value == 1 {
            ButtonState::Press
        } else {
            ButtonState::Release
        };
        self.send(Frame::Button { button, state });
    }

    fn on_rel(&mut self, axis: RelativeAxisType, value: i32) {
        if self.mode != Mode::Remote {
            return;
        }
        match axis {
            RelativeAxisType::REL_X => self.motion.0 += i64::from(value),
            RelativeAxisType::REL_Y => self.motion.1 += i64::from(value),
            RelativeAxisType::REL_WHEEL => self.wheel.1 += i64::from(value),
            RelativeAxisType::REL_HWHEEL => self.wheel.0 += i64::from(value),
            _ => {}
        }
    }

    /// 每个上报周期把累加的相对量发出去——一次 SYN 一帧,而非每个事件一帧。
    fn flush_motion(&mut self) {
        if self.mode != Mode::Remote {
            return;
        }
        if self.motion != (0, 0) {
            let (dx, dy) = (
                self.motion.0.clamp(i16::MIN.into(), i16::MAX.into()) as i16,
                self.motion.1.clamp(i16::MIN.into(), i16::MAX.into()) as i16,
            );
            self.send(Frame::Motion { dx, dy });
            self.motion = (0, 0);
        }
        if self.wheel != (0, 0) {
            let (dx, dy) = (
                self.wheel.0.clamp(i8::MIN.into(), i8::MAX.into()) as i8,
                self.wheel.1.clamp(i8::MIN.into(), i8::MAX.into()) as i8,
            );
            self.send(Frame::Wheel { dx, dy });
            self.wheel = (0, 0);
        }
    }

    fn toggle_mode(&mut self) {
        match self.mode {
            Mode::Local => {
                if self.conn.is_none() {
                    eprintln!("尚无客户端连接,保持本地模式");
                    return;
                }
                if let Err(e) = self.kbd.grab().and_then(|()| self.mouse.grab()) {
                    eprintln!("grab 失败,保持本地模式: {e}");
                    self.ungrab_all();
                    return;
                }
                self.mode = Mode::Remote;
                eprintln!("→ 远程模式:输入发往 macOS");
            }
            Mode::Remote => {
                // 先让对端释放所有按键,再松开 grab,顺序不能反。
                self.send(Frame::ReleaseAll);
                self.leave_remote();
                eprintln!("→ 本地模式");
            }
        }
    }

    fn leave_remote(&mut self) {
        if self.mode == Mode::Remote {
            self.ungrab_all();
            self.mode = Mode::Local;
        }
        self.motion = (0, 0);
        self.wheel = (0, 0);
    }

    fn ungrab_all(&mut self) {
        let _ = self.kbd.ungrab();
        let _ = self.mouse.ungrab();
    }

    fn send(&self, frame: Frame) {
        if let Some(conn) = &self.conn {
            conn.send(frame);
        }
    }
}

/// 按名字子串匹配设备;未指定时按能力自动探测:
/// 键盘要求有字母/回车/空格,鼠标要求有相对轴和左键。
/// 键盘候选中优先 keyd 虚拟设备(本地改键的下游,拿到的是改后的确定结果)。
fn open_device(kind: DeviceKind, name_filter: Option<&str>) -> io::Result<(Device, PathBuf)> {
    let filter = name_filter.map(str::to_lowercase);
    let mut fallback: Option<(Device, PathBuf)> = None;
    for (path, dev) in evdev::enumerate() {
        let name = dev.name().unwrap_or("").to_lowercase();
        let matches = match &filter {
            Some(f) => name.contains(f.as_str()),
            None => match kind {
                DeviceKind::Keyboard => is_keyboard(&dev),
                DeviceKind::Mouse => is_mouse(&dev),
            },
        };
        if !matches {
            continue;
        }
        if filter.is_none() && matches!(kind, DeviceKind::Keyboard) && name.contains("keyd virtual")
        {
            return Ok((dev, path));
        }
        if fallback.is_none() {
            fallback = Some((dev, path));
        }
    }
    fallback.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("找不到匹配的{}设备", kind.name()),
        )
    })
}

fn is_keyboard(dev: &Device) -> bool {
    dev.supported_keys().is_some_and(|keys| {
        keys.contains(Key::KEY_Q) && keys.contains(Key::KEY_ENTER) && keys.contains(Key::KEY_SPACE)
    })
}

fn is_mouse(dev: &Device) -> bool {
    dev.supported_relative_axes().is_some_and(|axes| {
        axes.contains(RelativeAxisType::REL_X) && axes.contains(RelativeAxisType::REL_Y)
    }) && dev.supported_keys().is_some_and(|keys| keys.contains(Key::BTN_LEFT))
}

/// fetch_events 的迭代器借用设备本身;收成 Vec 再返回,
/// 调用方才能一边持有事件一边操作 self。
fn fetch_all(dev: &mut Device) -> io::Result<Vec<evdev::InputEvent>> {
    Ok(dev.fetch_events()?.collect())
}

/// evdev 0.12 未暴露非阻塞设置,直接对 fd 做 fcntl。
fn set_nonblocking(dev: &Device) -> io::Result<()> {
    use nix::fcntl::{fcntl, FcntlArg, OFlag};
    fcntl(dev.as_raw_fd(), FcntlArg::F_SETFL(OFlag::O_NONBLOCK))
        .map_err(io::Error::from)?;
    Ok(())
}
