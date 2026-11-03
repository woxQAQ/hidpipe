//! Passive Linux wheel diagnostics. No device grab or input injection.
use std::env;
use std::io::{self, Write};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use evdev::{raw_stream::RawDevice, InputEventKind, RelativeAxisType, Synchronization};

const USAGE: &str = "Usage: scroll --list
       scroll /dev/input/eventN

Listen to one physical or virtual wheel device without grabbing it.
Pairs of ordinary and high-resolution events describe the same movement;
do not add them together. 120 high-resolution units represent one notch.
Device access may require sudo. Ctrl-C stops listening.";

const AXES: [RelativeAxisType; 4] = [
    RelativeAxisType::REL_WHEEL,
    RelativeAxisType::REL_HWHEEL,
    RelativeAxisType::REL_WHEEL_HI_RES,
    RelativeAxisType::REL_HWHEEL_HI_RES,
];

fn has_wheel(device: &RawDevice) -> bool {
    device
        .supported_relative_axes()
        .is_some_and(|axes| AXES.iter().any(|axis| axes.contains(*axis)))
}

fn list_devices() -> io::Result<()> {
    let mut devices: Vec<_> = evdev::raw_stream::enumerate()
        .filter(|(_, device)| has_wheel(device))
        .collect();
    devices.sort_by(|a, b| a.0.cmp(&b.0));
    if devices.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no readable wheel devices; try sudo if /dev/input access is restricted",
        ));
    }
    for (path, device) in devices {
        let axes = device.supported_relative_axes().unwrap();
        let supported: Vec<_> = AXES.iter().filter(|axis| axes.contains(**axis)).collect();
        println!(
            "{} name={:?} axes={supported:?}",
            path.display(),
            device.name().unwrap_or("unknown")
        );
    }
    Ok(())
}

fn listen(path: &str) -> io::Result<()> {
    let mut device = RawDevice::open(path)?;
    if !has_wheel(&device) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "device does not advertise relative wheel axes; use --list",
        ));
    }
    eprintln!(
        "Listening: {path} name={:?}",
        device.name().unwrap_or("unknown")
    );
    eprintln!(
        "120 HI_RES units = one notch. Ordinary and HI_RES values are separate representations."
    );
    eprintln!(
        "An exclusively grabbed physical device may be silent; try the OpenLogi virtual device."
    );
    eprintln!("Ctrl-C to stop.");

    let started = Instant::now();
    let mut previous: Option<SystemTime> = None;
    let mut frame = Vec::new();
    let mut dropped = false;
    let mut count = 0_u64;
    let stdout = io::stdout();
    let mut output = stdout.lock();
    loop {
        // RawDevice preserves SYN_DROPPED instead of synthesizing recovered state.
        for event in device.fetch_events()? {
            match event.kind() {
                InputEventKind::Synchronization(Synchronization::SYN_DROPPED) => {
                    frame.clear();
                    previous = None;
                    dropped = true;
                    eprintln!("SYN_DROPPED: events lost; discarding through the next SYN_REPORT");
                }
                InputEventKind::Synchronization(Synchronization::SYN_REPORT) => {
                    if dropped {
                        dropped = false;
                        continue;
                    }
                    if frame.is_empty() {
                        continue;
                    }
                    let timestamp = event.timestamp();
                    let interval = previous
                        .and_then(|last| timestamp.duration_since(last).ok())
                        .map(|dt| format!("{:.3}", dt.as_secs_f64() * 1000.0))
                        .unwrap_or_else(|| "-".into());
                    previous = Some(timestamp);
                    count += 1;
                    let timestamp_us = timestamp
                        .duration_since(UNIX_EPOCH)
                        .map(|dt| dt.as_micros())
                        .unwrap_or_default();
                    write!(
                        output,
                        "#{count} t={:.3}s dt={interval}ms timestamp_us={timestamp_us}",
                        started.elapsed().as_secs_f64()
                    )?;
                    for (axis, value) in frame.drain(..) {
                        write!(output, " {axis:?}={value}")?;
                    }
                    writeln!(output)?;
                    output.flush()?;
                }
                InputEventKind::RelAxis(axis) if !dropped && AXES.contains(&axis) => {
                    frame.push((axis, event.value()));
                }
                _ => {}
            }
        }
    }
}

fn run() -> io::Result<()> {
    let args: Vec<_> = env::args().skip(1).collect();
    match args.as_slice() {
        [] => list_devices(),
        [arg] if arg == "--list" => list_devices(),
        [arg] if arg == "--help" || arg == "-h" => {
            println!("{USAGE}");
            Ok(())
        }
        [path] if !path.starts_with('-') => listen(path),
        _ => Err(io::Error::new(io::ErrorKind::InvalidInput, USAGE)),
    }
}

fn main() {
    if let Err(error) = run() {
        if error.kind() == io::ErrorKind::BrokenPipe {
            return;
        }
        eprintln!("scroll: {error}");
        std::process::exit(1);
    }
}
