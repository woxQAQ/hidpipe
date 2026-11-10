//! hidpipe-client: macOS injection side.
//!
//! Connects to the Linux capture side and injects HID frames into the
//! system via CGEventPost. Injection happens at the Quartz layer, below
//! Karabiner's driver layer: local remap rules never touch remote keys (no
//! double mapping), and remote keys do not depend on local rules (no
//! flakiness). Remapping responsibility stays on the capture side, exactly
//! once.
//!
//! Modifier discipline (the core pitfall of macOS injection):
//! synthetic events inherit the event source's flags by default, and the
//! system updates its modifier state from the flags carried by each event.
//! Posting down/up without controlling flags corrupts modifier state
//! (symptoms: every key arrives with Control, left click becomes
//! ctrl+click and pops the context menu). So we keep our own modifier
//! bitmask and set_flags explicitly on every event.
//!
//! Three lines of defense against stuck keys:
//! 1. On disconnect, the Injector's Drop releases the keys held by this
//!    session;
//! 2. SIGTERM/SIGINT release via the shared PressedState from the signal
//!    thread;
//! 3. Every event's flags are explicitly controlled, so system-level
//!    leftovers cannot affect our events.

#![deny(unsafe_code)]

#[allow(unsafe_code)]
mod clipboard;
mod diagnostics;
#[allow(unsafe_code)]
mod display_cache;
#[allow(unsafe_code)]
mod event_access;
mod scroll;
mod table;
mod transport;

use std::collections::{HashSet, VecDeque};
use std::io;
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::{Duration, Instant};

use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGKeyCode, CGMouseButton, EventField,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;
use display_cache::{DisplayCache, Geometry};

use diagnostics::{measure, Stage};
use hidpipe_proto as proto;
use proto::{bridge::Resolver, ButtonState, Frame, KeyState, MouseButton, PointerEvent};
use scroll::{Scroll, ScrollModifier};

const READ_TIMEOUT: Duration = Duration::from_secs(6);
const RETRY_INTERVAL: Duration = Duration::from_secs(1);
/// CapsLock usage ID on HID page 0x07.
const HID_CAPS_LOCK: u16 = 0x39;
/// Return trigger: while the cursor is clamped at the left edge, control
/// is handed back only if the accumulated leftward push within the time
/// window exceeds the threshold. We sum over a sliding window instead of
/// resetting on any rightward move: a high-resolution mouse pressed against
/// the edge emits constant ±1 jitter, and a hard reset would turn the
/// trigger into pure luck.
const RETURN_PUSH_THRESHOLD: i32 = 500;
const RETURN_PUSH_WINDOW: Duration = Duration::from_millis(1500);

/// Runtime log with a local timestamp, format [HH:MM:SS].
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
    if args.clipboard {
        proto::clipboard::spawn_client(
            args.host.clone(),
            args.clipboard_port,
            clipboard::MacClipboard,
        );
    }
    let display = DisplayCache::main_display().unwrap_or_else(|error| {
        log!("failed to initialize display geometry: {error}");
        std::process::exit(1);
    });
    let geometry = display.snapshot();
    log!(
        "cached main display: id={} bounds={:?}",
        geometry.display_id,
        geometry.bounds
    );
    // The set of held keys is plain sendable data, shared with the signal
    // thread as a fallback; CGEventSource is not sendable, so the signal
    // thread builds its own.
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
        .expect("failed to install signal handler");
    }
    let mut requested_access = false;
    loop {
        if !event_access::is_allowed() {
            if !requested_access {
                let executable =
                    std::env::current_exe().unwrap_or_else(|_| "hidpipe-client".into());
                log!(
                    "macOS blocks input injection; enable Accessibility for {}. Waiting without connecting to the server",
                    executable.display()
                );
                event_access::request();
                requested_access = true;
            }
            sleep(RETRY_INTERVAL);
            continue;
        }
        if requested_access {
            log!("macOS input injection access granted");
            requested_access = false;
        }
        match session(&args, Arc::clone(&pressed), display) {
            Ok(()) => log!("peer closed the connection"),
            Err(e) => log!("connection interrupted: {e}"),
        }
        sleep(RETRY_INTERVAL);
    }
}

fn session(
    args: &Args,
    pressed: Arc<Mutex<PressedState>>,
    display: &'static DisplayCache,
) -> io::Result<()> {
    let mut resolver = args
        .openlogi_socket
        .as_ref()
        .map(|path| Resolver::connect(path))
        .transpose()?;
    let mut stream = TcpStream::connect((args.host.as_str(), args.port))?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    stream.set_write_timeout(Some(READ_TIMEOUT))?;
    proto::write_frame(
        &mut stream,
        &Frame::Hello {
            version: proto::VERSION,
            openlogi: resolver.is_some(),
        },
    )?;
    log!("connected to {}:{}", args.host, args.port);

    log!(
        "input feedback: {}; latency stats: {}",
        args.input_feedback,
        args.latency_stats
    );
    let mut injector = Injector::new(pressed, display, args.scroll);
    transport::run(
        &mut stream,
        resolver.as_mut(),
        args.input_feedback,
        args.latency_stats,
        |frame| {
            let kind = match frame {
                Frame::Motion { .. } | Frame::Pointer(PointerEvent::Motion { .. }) => "motion",
                Frame::Key { .. } => "key",
                Frame::Button { .. } | Frame::Pointer(PointerEvent::Button { .. }) => "button",
                Frame::Wheel { .. } | Frame::Pointer(PointerEvent::Wheel { .. }) => "wheel",
                _ => "control",
            };
            let _injection = diagnostics::Injection::begin(args.latency_stats, kind);
            injector.handle(frame)
        },
        event_access::is_allowed,
    )
}

/// The currently held keys, buttons and modifier bitmask. Plain data,
/// sendable across threads; this is the structural precondition for the
/// signal handler fallback.
#[derive(Default)]
struct PressedState {
    keys: HashSet<u16>,
    buttons: HashSet<MouseButton>,
    mods: CGEventFlags,
}

struct Injector {
    display: &'static DisplayCache,
    display_geometry: Geometry,
    source: CGEventSource,
    pressed: Arc<Mutex<PressedState>>,
    /// Scroll shaping applied to every injected wheel event.
    scroll: Scroll,
    /// Self-tracked cursor position. Querying the system position on every
    /// motion costs a WindowServer round trip per frame, which is measurable
    /// at 125Hz; within a session only we move the cursor, so self-tracking
    /// is enough. The initial value is read from the system once at session
    /// start.
    cursor: CGPoint,
    /// Time window of leftward pushes while the cursor is clamped at the
    /// left edge, (timestamp, displacement).
    edge_pushes: VecDeque<(Instant, i32)>,
}

impl Injector {
    fn new(
        pressed: Arc<Mutex<PressedState>>,
        display: &'static DisplayCache,
        scroll: ScrollModifier,
    ) -> Self {
        let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
            .expect("failed to create CGEventSource; grant this program access in System Settings > Privacy & Security > Accessibility");
        let display_geometry = display.snapshot();
        let cursor = current_location(&source).unwrap_or_else(|| {
            let b = display_geometry.bounds;
            CGPoint::new(
                b.origin.x + b.size.width / 2.0,
                b.origin.y + b.size.height / 2.0,
            )
        });
        Self {
            display,
            display_geometry,
            source,
            pressed,
            scroll: Scroll::new(scroll),
            cursor,
            edge_pushes: VecDeque::new(),
        }
    }

    /// Handle one frame; returns a frame to send back if a reply is needed
    /// (such as a return-to-local trigger).
    fn handle(&mut self, frame: Frame) -> Option<Frame> {
        match frame {
            Frame::Key { usage, state } => self.key(usage, state),
            Frame::Pointer(event) => return self.pointer(event),
            Frame::Motion { dx, dy } => return self.motion(i32::from(dx), i32::from(dy)),
            Frame::Button { button, state } => self.button(button, state),
            Frame::Wheel { .. } => self.wheel(&frame),
            Frame::ReleaseAll => self.release_all(),
            _ => {}
        }
        None
    }

    fn key(&mut self, usage: u16, state: KeyState) {
        // CapsLock is a locking modifier: we post no key event, we only
        // toggle the AlphaShift flag and let later events carry it. macOS
        // toggles the lock state once on each synthetic CapsLock down and
        // up, so carrying the flag yields exactly one case switch.
        if usage == HID_CAPS_LOCK {
            if state == KeyState::Press {
                self.pressed
                    .lock()
                    .unwrap()
                    .mods
                    .toggle(CGEventFlags::CGEventFlagAlphaShift);
            }
            return;
        }
        let Some(code) = table::hid_to_mac(usage) else {
            log!("unmapped HID usage ignored: {usage:#06x}");
            return;
        };
        let mut pressed = measure(Stage::State, || self.pressed.lock().unwrap());
        // Update the modifier bitmask first, then let the event carry the
        // flags as of after the event: a modifier's press event includes
        // its own flag, its release event does not.
        if let Some(flag) = modifier_flag(usage) {
            match state {
                KeyState::Press | KeyState::Repeat => pressed.mods.insert(flag),
                KeyState::Release => pressed.mods.remove(flag),
            }
        }
        post_key(
            &self.source,
            code,
            state != KeyState::Release,
            state == KeyState::Repeat,
            pressed.mods,
        );
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

    fn pointer(&mut self, event: PointerEvent) -> Option<Frame> {
        match event {
            PointerEvent::Motion { dx, dy } => return self.motion(dx, dy),
            PointerEvent::Button { button, pressed } => {
                let button = match button {
                    0 => MouseButton::Left,
                    1 => MouseButton::Right,
                    2 => MouseButton::Middle,
                    3 => MouseButton::Back,
                    4 => MouseButton::Forward,
                    _ => return None,
                };
                self.button(
                    button,
                    if pressed {
                        ButtonState::Press
                    } else {
                        ButtonState::Release
                    },
                );
            }
            PointerEvent::Wheel { .. } => self.wheel(&Frame::Pointer(event)),
            PointerEvent::Reset => self.release_all(),
            _ => {}
        }
        None
    }

    fn motion(&mut self, dx: i32, dy: i32) -> Option<Frame> {
        // v0 clamps to the main display only; multi-monitor is on the
        // roadmap.
        let geometry = measure(Stage::Bounds, || self.display.snapshot());
        if geometry != self.display_geometry {
            self.edge_pushes.clear();
            self.display_geometry = geometry;
        }
        let b = geometry.bounds;
        self.cursor.x =
            (self.cursor.x + f64::from(dx)).clamp(b.origin.x, b.origin.x + b.size.width - 1.0);
        self.cursor.y =
            (self.cursor.y + f64::from(dy)).clamp(b.origin.y, b.origin.y + b.size.height - 1.0);

        let (held_button, mods) = {
            let pressed = measure(Stage::State, || self.pressed.lock().unwrap());
            (pressed.buttons.iter().next().copied(), pressed.mods)
        };
        // Movement while a button is held must be posted as a drag event,
        // otherwise drag-select and drag-and-drop do not work on macOS.
        let event_type = held_button.map_or(CGEventType::MouseMoved, |button| match button {
            MouseButton::Left => CGEventType::LeftMouseDragged,
            MouseButton::Right => CGEventType::RightMouseDragged,
            MouseButton::Middle | MouseButton::Back | MouseButton::Forward => {
                CGEventType::OtherMouseDragged
            }
        });
        let Ok(event) = measure(Stage::Create, || {
            CGEvent::new_mouse_event(
                self.source.clone(),
                event_type,
                self.cursor,
                CGMouseButton::Left,
            )
        }) else {
            return None;
        };
        // Games and other readers of relative motion look at the delta
        // fields, not the absolute position.
        measure(Stage::Configure, || {
            event.set_integer_value_field(EventField::MOUSE_EVENT_DELTA_X, i64::from(dx));
            event.set_integer_value_field(EventField::MOUSE_EVENT_DELTA_Y, i64::from(dy));
            event.set_integer_value_field(
                EventField::MOUSE_EVENT_BUTTON_NUMBER,
                held_button.map_or(0, |button| button as i64),
            );
            event.set_flags(mods);
        });
        measure(Stage::Post, || event.post(CGEventTapLocation::HID));
        measure(Stage::Drop, || drop(event));

        // Return edge detection: while the cursor is clamped at the left
        // edge with no button held, leftward pushes enter the time window.
        // Rightward motion simply does not enter the window and does not
        // reset it (tolerates hand jitter). The window is cleared only once
        // the cursor leaves the left edge (normal inward use).
        let pinned = self.cursor.x <= b.origin.x && held_button.is_none();
        if pinned {
            if dx < 0 {
                self.edge_pushes
                    .push_back((Instant::now(), dx.saturating_neg()));
            }
            while self
                .edge_pushes
                .front()
                .is_some_and(|(t, _)| t.elapsed() > RETURN_PUSH_WINDOW)
            {
                self.edge_pushes.pop_front();
            }
            let pushed = self
                .edge_pushes
                .iter()
                .fold(0i32, |sum, (_, v)| sum.saturating_add(*v));
            if pushed > RETURN_PUSH_THRESHOLD {
                self.edge_pushes.clear();
                log!("cursor pushed through the left edge, handing control back");
                return Some(Frame::ReturnToLocal);
            }
        } else {
            self.edge_pushes.clear();
        }
        None
    }

    fn button(&mut self, button: MouseButton, state: ButtonState) {
        let mut pressed = measure(Stage::State, || self.pressed.lock().unwrap());
        post_button(&self.source, button, state, self.cursor, pressed.mods);
        match state {
            ButtonState::Press => {
                pressed.buttons.insert(button);
            }
            ButtonState::Release => {
                pressed.buttons.remove(&button);
            }
        }
    }

    fn wheel(&mut self, frame: &Frame) {
        let mods = measure(Stage::State, || self.pressed.lock().unwrap().mods);
        let Ok(Some(event)) = self.scroll.event(&self.source, frame, mods) else {
            return;
        };
        measure(Stage::Post, || event.post(CGEventTapLocation::HID));
        measure(Stage::Drop, || drop(event));
    }

    fn release_all(&mut self) {
        let _ = self
            .scroll
            .event(&self.source, &Frame::ReleaseAll, CGEventFlags::empty());
        self.edge_pushes.clear();
        release_all(&self.source, &mut self.pressed.lock().unwrap());
    }
}

impl Drop for Injector {
    fn drop(&mut self) {
        self.release_all();
    }
}

/// Release all held keys and buttons. Kept as a free function so both the
/// normal path (Injector) and the signal path (with its own source) share
/// it.
fn release_all(source: &CGEventSource, pressed: &mut PressedState) {
    for usage in pressed.keys.drain() {
        if let Some(flag) = modifier_flag(usage) {
            pressed.mods.remove(flag);
        }
        if let Some(code) = table::hid_to_mac(usage) {
            post_key(source, code, false, false, pressed.mods);
        }
    }
    pressed.mods = CGEventFlags::empty();
    let buttons: Vec<MouseButton> = pressed.buttons.drain().collect();
    for button in buttons {
        if let Some(p) = current_location(source) {
            post_button(source, button, ButtonState::Release, p, pressed.mods);
        }
    }
}

/// Modifier usage -> CGEventFlags. CapsLock (0x39) is not here; it is a
/// locking modifier and takes the AlphaShift toggle path.
fn modifier_flag(usage: u16) -> Option<CGEventFlags> {
    Some(match usage {
        0xE0 | 0xE4 => CGEventFlags::CGEventFlagControl,
        0xE1 | 0xE5 => CGEventFlags::CGEventFlagShift,
        0xE2 | 0xE6 => CGEventFlags::CGEventFlagAlternate,
        0xE3 | 0xE7 => CGEventFlags::CGEventFlagCommand,
        _ => return None,
    })
}

fn post_key(
    source: &CGEventSource,
    code: CGKeyCode,
    down: bool,
    autorepeat: bool,
    flags: CGEventFlags,
) {
    let Ok(event) = measure(Stage::Create, || {
        CGEvent::new_keyboard_event(source.clone(), code, down)
    }) else {
        log!("failed to create keyboard event: {code:#06x}");
        return;
    };
    let mut flags = flags;
    // Hardware arrow keys carry NumericPad|SecondaryFn; CGEventTap-based
    // hotkey tools (tiling window managers etc.) use these two flags to
    // recognize navigation keys.
    if (0x7B..=0x7E).contains(&code) {
        flags |= CGEventFlags::CGEventFlagNumericPad | CGEventFlags::CGEventFlagSecondaryFn;
    }
    measure(Stage::Configure, || {
        event.set_flags(flags);
        if autorepeat {
            event.set_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT, 1);
        }
    });
    measure(Stage::Post, || event.post(CGEventTapLocation::HID));
    measure(Stage::Drop, || drop(event));
}

fn post_button(
    source: &CGEventSource,
    button: MouseButton,
    state: ButtonState,
    position: CGPoint,
    flags: CGEventFlags,
) {
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
        (MouseButton::Back | MouseButton::Forward, ButtonState::Press) => {
            (CGEventType::OtherMouseDown, CGMouseButton::Center)
        }
        (MouseButton::Back | MouseButton::Forward, ButtonState::Release) => {
            (CGEventType::OtherMouseUp, CGMouseButton::Center)
        }
    };
    let Ok(event) = measure(Stage::Create, || {
        CGEvent::new_mouse_event(source.clone(), event_type, position, cg_button)
    }) else {
        return;
    };
    measure(Stage::Configure, || {
        event.set_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER, button as i64);
        event.set_flags(flags);
    });
    measure(Stage::Post, || event.post(CGEventTapLocation::HID));
    measure(Stage::Drop, || drop(event));
}

fn current_location(source: &CGEventSource) -> Option<CGPoint> {
    CGEvent::new(source.clone()).ok().map(|e| e.location())
}

struct Args {
    host: String,
    port: u16,
    clipboard: bool,
    clipboard_port: u16,
    input_feedback: bool,
    latency_stats: bool,
    openlogi_socket: Option<std::path::PathBuf>,
    scroll: ScrollModifier,
}

const USAGE: &str = "\
hidpipe-client: macOS input injection side

Usage: hidpipe-client <server-address> [options]

Options:
  --port <port>   server port (default 24800)
  --openlogi-socket <path>  resolve mouse input with the local OpenLogi agent
  --clipboard    share text and images over a separate trusted-LAN connection
  --clipboard-port <port>  clipboard port (default 24801; requires --clipboard)
  --input-feedback  reply to every input frame to mitigate TCP delayed ACK
  --latency-stats   log local timing summaries every 5 seconds
  --invert-scroll-x   flip horizontal scroll direction
  --invert-scroll-y   flip vertical scroll direction
  --scroll-scale-x <f>  multiply horizontal scroll speed (default 1.0)
  --scroll-scale-y <f>  multiply vertical scroll speed (default 1.0)
  -h, --help      show this help

Grant this executable Accessibility access in System Settings > Privacy & Security > Accessibility.
A launchd service needs its own permission; the terminal's permission does not cover it.
";

impl Args {
    fn parse() -> Self {
        let mut args = Self {
            host: String::new(),
            port: 24800,
            clipboard: false,
            clipboard_port: 24801,
            input_feedback: false,
            latency_stats: false,
            openlogi_socket: None,
            scroll: ScrollModifier {
                scale_x: 1.0,
                scale_y: 1.0,
                invert_x: false,
                invert_y: false,
            },
        };
        let mut it = std::env::args().skip(1);
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--port" => {
                    let value = it.next().unwrap_or_else(|| {
                        eprintln!("--port requires a value\n{USAGE}");
                        std::process::exit(2);
                    });
                    args.port = value.parse().unwrap_or_else(|_| {
                        eprintln!("--port is not a valid port number\n{USAGE}");
                        std::process::exit(2);
                    });
                }
                "--openlogi-socket" => {
                    args.openlogi_socket = Some(
                        it.next()
                            .unwrap_or_else(|| {
                                eprintln!("--openlogi-socket requires a path\n{USAGE}");
                                std::process::exit(2);
                            })
                            .into(),
                    );
                }
                "--clipboard" => args.clipboard = true,
                "--clipboard-port" => {
                    let value = it.next().unwrap_or_else(|| {
                        eprintln!("--clipboard-port requires a value\n{USAGE}");
                        std::process::exit(2);
                    });
                    args.clipboard_port = value.parse().unwrap_or_else(|_| {
                        eprintln!("--clipboard-port is not a valid port number\n{USAGE}");
                        std::process::exit(2);
                    });
                }
                "--input-feedback" => args.input_feedback = true,
                "--latency-stats" => args.latency_stats = true,
                "--invert-scroll-x" => args.scroll.invert_x = true,
                "--invert-scroll-y" => args.scroll.invert_y = true,
                "--scroll-scale-x" => {
                    args.scroll.scale_x = parse_scale("--scroll-scale-x", it.next());
                }
                "--scroll-scale-y" => {
                    args.scroll.scale_y = parse_scale("--scroll-scale-y", it.next());
                }
                "-h" | "--help" => {
                    print!("{USAGE}");
                    std::process::exit(0);
                }
                other if other.starts_with('-') => {
                    eprintln!("unknown argument {other}\n{USAGE}");
                    std::process::exit(2);
                }
                other => args.host = other.to_string(),
            }
        }
        if args.clipboard && args.clipboard_port == args.port {
            eprintln!("clipboard and input ports must differ");
            std::process::exit(2);
        }
        if args.host.is_empty() {
            eprintln!("missing server address\n{USAGE}");
            std::process::exit(2);
        }
        args
    }
}

fn parse_scale(flag: &str, value: Option<String>) -> f64 {
    let value = value.unwrap_or_else(|| {
        eprintln!("{flag} requires a value\n{USAGE}");
        std::process::exit(2);
    });
    let scale = value.parse::<f64>().unwrap_or_else(|_| {
        eprintln!("{flag} is not a number\n{USAGE}");
        std::process::exit(2);
    });
    if !scale.is_finite() || scale < 0.0 {
        eprintln!("{flag} must be a finite, non-negative number\n{USAGE}");
        std::process::exit(2);
    }
    scale
}
