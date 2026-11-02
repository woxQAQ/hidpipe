//! Linux uinput integration probe. Run with:
//! cargo run -p openlogi-hook --example hidpipe-capture
//! The fixture never emits input until its source has been exclusively grabbed.
//! Set LIBINPUT_DEBUG_EVENTS to the libinput executable to verify desktop delivery.

use std::error::Error;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use evdev::uinput::VirtualDevice;
use evdev::{
    AttributeSet, BusType, Device, EventType, InputEvent, InputId, KeyCode, RelativeAxisCode,
};
use openlogi_hook::{EventDisposition, Hook, HookError, HookEvent, MouseEvent};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn fixture_id() -> InputId {
    InputId::new(BusType::BUS_VIRTUAL, 0x046d, 0xfffe, 1)
}

fn device_named(name: &str) -> Result<Device> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some((_, device)) = evdev::enumerate()
            .find(|(_, device)| device.name() == Some(name) && device.input_id() == fixture_id())
        {
            return Ok(device);
        }
        if Instant::now() >= deadline {
            return Err(format!("device not readable with fixture identity: {name}").into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn worker(stage: &str) -> Result<()> {
    if stage.starts_with("wheel-") {
        return wheel_worker(stage == "wheel-hires");
    }
    if stage == "busy" {
        match Hook::start(|_| EventDisposition::PassThrough) {
            Err(HookError::Linux(error)) if error.raw_os_error() == Some(16) => return Ok(()),
            _ => return Err("hook reported ready without exclusive ownership".into()),
        }
    }
    let (sender, receiver) = mpsc::sync_channel(8);
    let _hook = Hook::start(move |event| {
        let _ = sender.try_send(event);
        EventDisposition::Suppress
    })?;
    // This must identify the kernel's real virtual device, not just a builder.
    let _virtual_mouse = device_named("OpenLogi virtual mouse")?;
    println!("capture-ready");
    std::io::stdout().flush()?;
    let event = receiver.recv_timeout(Duration::from_secs(3))?;
    if !matches!(
        event,
        HookEvent::Mouse(MouseEvent::Moved {
            delta_x: 37,
            delta_y: 0
        })
    ) {
        return Err(format!("unexpected captured event: {event:?}").into());
    }
    Ok(())
}

fn desktop_probe() -> Result<Option<(std::process::Child, mpsc::Receiver<String>)>> {
    let Some(tool) = std::env::var_os("LIBINPUT_DEBUG_EVENTS") else {
        return Ok(None);
    };
    let (path, _) = evdev::enumerate()
        .find(|(_, device)| {
            device.name() == Some("OpenLogi virtual mouse") && device.input_id() == fixture_id()
        })
        .ok_or("virtual mouse missing for libinput")?;
    let mut process = Command::new("timeout")
        .args(["8s", "stdbuf", "-oL"])
        .arg(tool)
        .arg("debug-events")
        .arg("--device")
        .arg(path)
        .stdout(Stdio::piped())
        .spawn()?;
    let stdout = process.stdout.take().ok_or("missing libinput output")?;
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout)
            .lines()
            .map_while(std::result::Result::ok)
        {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    loop {
        let line = receiver.recv_timeout(Duration::from_secs(3))?;
        if line.contains("DEVICE_ADDED") {
            break;
        }
    }
    Ok(Some((process, receiver)))
}

fn wheel_worker(hires: bool) -> Result<()> {
    let (sender, receiver) = mpsc::sync_channel(32);
    let _hook = Hook::start(move |event| {
        let _ = sender.try_send(event);
        EventDisposition::PassThrough
    })?;
    let mut virtual_mouse = device_named("OpenLogi virtual mouse")?;
    let axes = virtual_mouse
        .supported_relative_axes()
        .ok_or("missing axes")?;
    if !axes.contains(RelativeAxisCode::REL_WHEEL_HI_RES)
        || !axes.contains(RelativeAxisCode::REL_HWHEEL_HI_RES)
    {
        return Err("virtual wheel capabilities disagree with delivery policy".into());
    }
    let desktop = desktop_probe()?;
    println!("capture-ready");
    std::io::stdout().flush()?;
    let mut scrolls = Vec::new();
    loop {
        match receiver.recv_timeout(Duration::from_secs(3))? {
            HookEvent::Mouse(MouseEvent::Scroll {
                delta_x, delta_y, ..
            }) => {
                scrolls.push((delta_x, delta_y));
            }
            HookEvent::Mouse(MouseEvent::Moved { delta_x: 37, .. }) => break,
            event => return Err(format!("unexpected wheel probe event: {event:?}").into()),
        }
    }
    let expected = if hires {
        vec![
            (0.0, 1.0 / 120.0),
            (0.0, 119.0 / 120.0),
            (0.0, -1.0 / 120.0),
            (0.0, -119.0 / 120.0),
            (0.0, 1.0),
            (0.5, 0.0),
            (0.0, 0.5),
            (0.0, 0.5),
            (0.0, 0.5),
        ]
    } else {
        vec![(0.0, 1.0), (0.0, -1.0), (0.0, 1.0), (0.5, 0.0), (0.0, 1.0)]
    };
    if scrolls != expected {
        return Err(format!("callback wheel stream: {scrolls:?}, expected {expected:?}").into());
    }
    virtual_mouse.set_nonblocking(true)?;
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut delivered = Vec::new();
    'reports: loop {
        let events = match virtual_mouse.fetch_events() {
            Ok(events) => events,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err("virtual wheel delivery timed out".into());
                }
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        for event in events {
            if event.event_type() == EventType::RELATIVE {
                if event.code() == RelativeAxisCode::REL_X.0 {
                    break 'reports;
                }
                delivered.push((event.code(), event.value()));
            }
        }
    }
    let expected = if hires {
        vec![
            (RelativeAxisCode::REL_WHEEL_HI_RES.0, 1),
            (RelativeAxisCode::REL_WHEEL_HI_RES.0, 119),
            (RelativeAxisCode::REL_WHEEL_HI_RES.0, -1),
            (RelativeAxisCode::REL_WHEEL_HI_RES.0, -119),
            (RelativeAxisCode::REL_WHEEL_HI_RES.0, 120),
            (RelativeAxisCode::REL_HWHEEL_HI_RES.0, 60),
            (RelativeAxisCode::REL_WHEEL_HI_RES.0, 60),
            (RelativeAxisCode::REL_WHEEL_HI_RES.0, 60),
            (RelativeAxisCode::REL_WHEEL_HI_RES.0, 60),
        ]
    } else {
        vec![
            (RelativeAxisCode::REL_WHEEL_HI_RES.0, 120),
            (RelativeAxisCode::REL_WHEEL_HI_RES.0, -120),
            (RelativeAxisCode::REL_WHEEL_HI_RES.0, 120),
            (RelativeAxisCode::REL_HWHEEL_HI_RES.0, 60),
            (RelativeAxisCode::REL_WHEEL_HI_RES.0, 120),
        ]
    };
    if delivered != expected {
        return Err(format!("local wheel stream: {delivered:?}, expected {expected:?}").into());
    }
    finish_desktop_probe(desktop)
}

fn finish_desktop_probe(
    desktop: Option<(std::process::Child, mpsc::Receiver<String>)>,
) -> Result<()> {
    if let Some((mut process, receiver)) = desktop {
        let result: Result<()> = loop {
            match receiver.recv_timeout(Duration::from_secs(3)) {
                Ok(line)
                    if line.contains("POINTER_SCROLL_WHEEL")
                        && line
                            .split_once("vert ")
                            .and_then(|(_, axes)| axes.split('/').next())
                            .and_then(|delta| delta.parse::<f64>().ok())
                            .is_some_and(|delta| delta != 0.0) =>
                {
                    eprintln!("libinput accepted vertical wheel: {line}");
                    break Ok(());
                }
                Ok(_) => {}
                Err(error) => break Err(error.into()),
            }
        };
        let _ = process.kill();
        let _ = process.wait();
        result?;
    }
    Ok(())
}

fn child(stage: &str, name: &str) -> Result<Command> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg(stage)
        .env(
            "OPENLOGI_BRIDGE_SOCKET",
            "/tmp/hidpipe-capture-fixture.sock",
        )
        .env("OPENLOGI_BRIDGE_MOUSE", name)
        .env(
            "OPENLOGI_BRIDGE_WHEEL_HIRES",
            if stage == "wheel-hires" { "1" } else { "0" },
        );
    Ok(command)
}

fn main() -> Result<()> {
    if let Some(stage) = std::env::args().nth(1) {
        return worker(&stage);
    }
    let name = format!("hidpipe capture fixture {}", std::process::id());
    let mut keys = AttributeSet::<KeyCode>::new();
    keys.insert(KeyCode::BTN_LEFT);
    let mut axes = AttributeSet::<RelativeAxisCode>::new();
    axes.insert(RelativeAxisCode::REL_X);
    axes.insert(RelativeAxisCode::REL_Y);
    axes.insert(RelativeAxisCode::REL_WHEEL);
    axes.insert(RelativeAxisCode::REL_WHEEL_HI_RES);
    axes.insert(RelativeAxisCode::REL_HWHEEL);
    axes.insert(RelativeAxisCode::REL_HWHEEL_HI_RES);
    let mut fixture = VirtualDevice::builder()?
        .name(&name)
        .input_id(fixture_id())
        .with_keys(&keys)?
        .with_relative_axes(&axes)?
        .build()?;
    let mut held = device_named(&name)?;
    held.grab()?;
    if !child("busy", &name)?.status()?.success() {
        return Err("exclusive-grab admission regression".into());
    }
    held.ungrab()?;
    for stage in ["capture", "wheel-discrete", "wheel-hires"] {
        let mut process = child(stage, &name)?.stdout(Stdio::piped()).spawn()?;
        let stdout = process.stdout.take().ok_or("missing child output")?;
        let mut line = String::new();
        BufReader::new(stdout).read_line(&mut line)?;
        if line.trim() != "capture-ready" {
            let _ = process.wait();
            return Err("capture worker failed before readiness".into());
        }
        if stage.starts_with("wheel-") {
            for (axis, value) in [
                (RelativeAxisCode::REL_WHEEL_HI_RES, 1),
                (RelativeAxisCode::REL_WHEEL_HI_RES, 119),
                (RelativeAxisCode::REL_WHEEL, 1),
                (RelativeAxisCode::REL_WHEEL_HI_RES, -1),
                (RelativeAxisCode::REL_WHEEL_HI_RES, -119),
                (RelativeAxisCode::REL_WHEEL, -1),
                // A source may report a complete notch without a coarse companion.
                (RelativeAxisCode::REL_WHEEL_HI_RES, 120),
                (RelativeAxisCode::REL_HWHEEL_HI_RES, 60),
                (RelativeAxisCode::REL_HWHEEL, 1),
            ] {
                fixture.emit(&[InputEvent::new(EventType::RELATIVE.0, axis.0, value)])?;
            }
            // Stale partial movement cannot contribute to a later gesture.
            fixture.emit(&[InputEvent::new(
                EventType::RELATIVE.0,
                RelativeAxisCode::REL_WHEEL_HI_RES.0,
                60,
            )])?;
            std::thread::sleep(Duration::from_millis(300));
            for _ in 0..2 {
                fixture.emit(&[InputEvent::new(
                    EventType::RELATIVE.0,
                    RelativeAxisCode::REL_WHEEL_HI_RES.0,
                    60,
                )])?;
            }
        }
        fixture.emit(&[InputEvent::new(
            EventType::RELATIVE.0,
            RelativeAxisCode::REL_X.0,
            37,
        )])?;
        if !process.wait()?.success() {
            return Err(format!("{stage} capture regression").into());
        }
    }
    println!(
        "PASS: exclusive capture, device identity, motion, discrete and high-resolution wheel delivery"
    );
    Ok(())
}
