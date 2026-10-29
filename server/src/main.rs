//! hidpipe-server: Linux capture side.
//!
//! Responsibilities:
//! 1. Read raw input from evdev. The preferred keyboard source is the keyd
//!    virtual device, the definitive source after local remapping.
//! 2. Enter remote mode at the screen edge. In remote mode, EVIOCGRAB
//!    grabs keyboard and mouse, and events are translated into HID usage ID
//!    frames for the macOS injector.
//! 3. Ungrab on disconnect. Grab state is released when the connection dies,
//!    never left behind.

#![forbid(unsafe_code)]

#[macro_use]
mod log;
mod args;
mod barrier;
mod hid;
mod net;

use std::io;
use std::net::{SocketAddr, TcpListener};
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread::sleep;
use std::time::Duration;

use evdev::{Device, InputEventKind, Key, RelativeAxisType};
use hidpipe_proto::{ButtonState, Frame, KeyState, MouseButton};

use args::Args;
use barrier::BarrierEvent;
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
            DeviceKind::Keyboard => "keyboard",
            DeviceKind::Mouse => "mouse",
        }
    }
}

fn main() -> io::Result<()> {
    let args = Args::parse();
    let (barrier_tx, barrier_rx) = mpsc::channel::<BarrierEvent>();
    barrier::spawn(barrier_tx)?;
    let (kbd, kbd_path) = open_device(DeviceKind::Keyboard, args.keyboard.as_deref())?;
    let (mouse, mouse_path) = open_device(DeviceKind::Mouse, args.mouse.as_deref())?;
    log!(
        "keyboard: {} ({})",
        kbd.name().unwrap_or("?"),
        kbd_path.display()
    );
    log!(
        "mouse: {} ({})",
        mouse.name().unwrap_or("?"),
        mouse_path.display()
    );
    set_nonblocking(&kbd)?;
    set_nonblocking(&mouse)?;

    let listener = TcpListener::bind((args.bind.as_str(), args.port))?;
    listener.set_nonblocking(true)?;
    log!("listening on {}:{}", args.bind, args.port);

    let (net_tx, net_rx) = mpsc::channel::<NetMsg>();
    let mut app = App {
        kbd,
        mouse,
        conn: None,
        pending: None,
        conn_id: 0,
        mode: Mode::Local,
        motion: (0, 0),
        wheel: (0, 0),
        armed: true,
        swap_caps_esc: args.swap_caps_esc,
        listener,
        net_tx,
        net_rx,
        barrier_rx,
    };

    loop {
        app.accept_pending();
        app.handle_net_msgs();
        app.drain_barrier();
        app.drain_keyboard();
        app.drain_mouse();
        sleep(Duration::from_millis(1));
    }
}

struct App {
    kbd: Device,
    mouse: Device,
    /// Connection that passed the handshake and is now serving.
    conn: Option<Connection>,
    /// Freshly accepted connection that has not passed the handshake yet.
    /// It is kept separate from the serving connection and can only take
    /// over after a valid Hello.
    pending: Option<(Connection, SocketAddr)>,
    conn_id: u64,
    mode: Mode,
    /// Relative motion accumulated between SYN_REPORTs.
    motion: (i64, i64),
    wheel: (i64, i64),
    swap_caps_esc: bool,
    listener: TcpListener,
    net_tx: mpsc::Sender<NetMsg>,
    net_rx: mpsc::Receiver<NetMsg>,
    barrier_rx: mpsc::Receiver<BarrierEvent>,
    /// Barrier arming state: disarmed on entering remote mode, re-armed when
    /// the pointer leaves the barrier. Prevents an immediate bounce back to
    /// remote when the pointer still sits on the barrier after switching
    /// back to local.
    armed: bool,
}

impl App {
    /// A new connection waits in the pending slot for the handshake instead
    /// of taking over directly. A replaced pending connection dies on its
    /// own when its channel closes.
    fn accept_pending(&mut self) {
        while let Ok((stream, addr)) = self.listener.accept() {
            log!("pending connection: {addr}");
            self.conn_id += 1;
            match Connection::spawn(stream, self.net_tx.clone(), self.conn_id) {
                Ok(conn) => self.pending = Some((conn, addr)),
                Err(e) => log!("connection setup failed: {e}"),
            }
        }
    }

    fn handle_net_msgs(&mut self) {
        while let Ok(msg) = self.net_rx.try_recv() {
            match msg {
                NetMsg::Hello(id) => {
                    if self.pending.as_ref().is_some_and(|(c, _)| c.id == id) {
                        let (conn, addr) = self.pending.take().expect("checked above");
                        log!("client ready: {addr}");
                        // A handshaken connection is allowed to take over;
                        // this is the reconnect path after a client restart.
                        self.conn = Some(conn);
                    }
                }
                NetMsg::Dead(id) => {
                    if self.pending.as_ref().is_some_and(|(c, _)| c.id == id) {
                        log!("pending connection failed handshake, dropped");
                        self.pending = None;
                    }
                    if self.conn.as_ref().is_some_and(|c| c.id == id) {
                        log!("connection dead, back to local mode");
                        self.conn = None;
                        self.leave_remote();
                    }
                }
                NetMsg::ReturnToLocal(id) => {
                    if self.conn.as_ref().is_some_and(|c| c.id == id) && self.mode == Mode::Remote {
                        log!("peer edge trigger, handing control back");
                        self.send(Frame::ReleaseAll);
                        self.leave_remote();
                    }
                }
            }
        }
    }

    fn drain_barrier(&mut self) {
        // Collect events before handling, otherwise try_recv's immutable
        // borrow blocks &mut self.
        let mut events = Vec::new();
        while let Ok(event) = self.barrier_rx.try_recv() {
            events.push(event);
        }
        for event in events {
            match event {
                BarrierEvent::Enter => {
                    if self.armed && self.mode == Mode::Local {
                        self.armed = false;
                        self.enter_remote();
                    }
                }
                BarrierEvent::Leave => self.armed = true,
            }
        }
    }

    fn drain_keyboard(&mut self) {
        let events = match fetch_all(&mut self.kbd) {
            Ok(events) => events,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
            Err(e) => {
                // Hard errors such as an unplugged device: fall back to local
                // mode and wait for manual intervention, better than silently
                // losing input.
                log!("keyboard read failed ({e}), back to local mode");
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
                log!("mouse read failed ({e}), back to local mode");
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

    /// value: 0 = release, 1 = press, 2 = hold repeat.
    fn on_key(&mut self, key: Key, value: i32) {
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
                log!("unmapped key ignored: {key:?}");
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
            // Side buttons are not forwarded in v0.
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

    /// Send the accumulated relative motion once per report cycle: one frame
    /// per SYN, not one frame per event.
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

    fn enter_remote(&mut self) {
        if self.conn.is_none() {
            log!("no client connected, staying local");
            return;
        }
        if let Err(e) = self.kbd.grab().and_then(|()| self.mouse.grab()) {
            log!("grab failed, staying local: {e}");
            self.ungrab_all();
            return;
        }
        self.mode = Mode::Remote;
        log!("-> remote mode: input goes to macOS");
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

/// Match a device by name substring; without a filter, auto-detect by
/// capability: a keyboard must have letter/enter/space keys, a mouse must
/// have relative axes and a left button. Among keyboard candidates the keyd
/// virtual device wins, since it sits downstream of local remapping and
/// yields the final key codes.
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
            format!("no matching {} device found", kind.name()),
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
    }) && dev
        .supported_keys()
        .is_some_and(|keys| keys.contains(Key::BTN_LEFT))
}

/// fetch_events borrows the device for its iterator; collecting into a Vec
/// lets the caller hold the events while mutating self.
fn fetch_all(dev: &mut Device) -> io::Result<Vec<evdev::InputEvent>> {
    Ok(dev.fetch_events()?.collect())
}

/// evdev 0.12 does not expose nonblocking mode, so fcntl the fd directly.
fn set_nonblocking(dev: &Device) -> io::Result<()> {
    use nix::fcntl::{fcntl, FcntlArg, OFlag};
    fcntl(dev.as_raw_fd(), FcntlArg::F_SETFL(OFlag::O_NONBLOCK)).map_err(io::Error::from)?;
    Ok(())
}
