//! Linux uinput integration probe. Run with:
//! cargo run -p openlogi-hook --example hidpipe-capture
//! The fixture never emits input until its source has been exclusively grabbed.

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

fn child(stage: &str, name: &str) -> Result<Command> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg(stage)
        .env(
            "OPENLOGI_BRIDGE_SOCKET",
            "/tmp/hidpipe-capture-fixture.sock",
        )
        .env("OPENLOGI_BRIDGE_MOUSE", name);
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
    let mut process = child("capture", &name)?.stdout(Stdio::piped()).spawn()?;
    let stdout = process.stdout.take().ok_or("missing child output")?;
    let mut line = String::new();
    BufReader::new(stdout).read_line(&mut line)?;
    if line.trim() != "capture-ready" {
        let _ = process.wait();
        return Err("capture worker failed before readiness".into());
    }
    fixture.emit(&[InputEvent::new(
        EventType::RELATIVE.0,
        RelativeAxisCode::REL_X.0,
        37,
    )])?;
    if !process.wait()?.success() {
        return Err("motion capture regression".into());
    }
    println!(
        "PASS: busy devices reject readiness; successful capture preserves device identity and receives motion"
    );
    Ok(())
}
