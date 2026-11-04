//! Exercise side-button ownership across real bridge socket teardown.
#![cfg(target_os = "linux")]
#![expect(clippy::unwrap_used, reason = "integration test assertions")]

use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use openlogi_agent_core::{orchestrator::Orchestrator, remote};
use openlogi_core::binding::{Action, ButtonId, GestureDirection};
use openlogi_core::config::Config;
use openlogi_hid::gesture::CapturedInput;
use openlogi_hook::MouseEvent;
use remote::wire::{MAGIC, PointerEvent};

fn capture(socket: &Path) -> UnixStream {
    let mut stream = UnixStream::connect(socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream.write_all(&MAGIC).unwrap();
    stream.write_all(&[0]).unwrap();
    let mut ack = [1];
    stream.read_exact(&mut ack).unwrap();
    assert_eq!(ack, [0], "released controls must allow the next switch");
    stream
}

fn native(pressed: bool) -> MouseEvent {
    MouseEvent::Button {
        id: ButtonId::Back,
        pressed,
        device: None,
    }
}

fn gesture(pressed: bool) {
    assert!(remote::captured(
        "mouse",
        CapturedInput::RawButton {
            button: ButtonId::GestureButton,
            pressed,
        },
    ));
}

fn wait_local() {
    let deadline = Instant::now() + Duration::from_secs(2);
    while remote::hook_input(&MouseEvent::Moved {
        delta_x: 1,
        delta_y: 0,
    })
    .is_some()
    {
        assert!(Instant::now() < deadline, "EOF must restore local routing");
        std::thread::yield_now();
    }
}

#[test]
fn side_buttons_recover_when_release_races_with_return_to_local() {
    let directory =
        std::env::temp_dir().join(format!("openlogi-switch-recovery-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = directory.join("pointer.sock");
    let clicks = directory.join("clicks");
    let mut config = Config::default();
    // Observe completed local gestures through a temporary file, without
    // synthesizing keyboard/mouse input into the running desktop.
    config.set_gesture_direction(
        "mouse",
        ButtonId::GestureButton,
        GestureDirection::Click,
        Action::RunShellCommand(format!("printf x >> '{}'", clicks.display())),
    );
    let bridge = remote::install(
        remote::Options {
            socket: socket.clone(),
            device: "mouse".into(),
        },
        &config,
    )
    .unwrap();
    let orchestrator = Orchestrator::new(config);
    bridge.serve(orchestrator.shared()).unwrap();
    remote::hook_ready(true);
    assert!(remote::captured("mouse", CapturedInput::Ready));

    for cycle in 1..=128 {
        let mut stream = capture(&socket);
        assert!(remote::hook_input(&native(true)).is_some());
        gesture(true);
        // Reading both presses ensures they belong to this remote lease.
        for button in [3, 5] {
            assert_eq!(
                PointerEvent::read(&mut stream).unwrap(),
                PointerEvent::Button {
                    button,
                    pressed: true,
                }
            );
        }
        let start = Arc::new(Barrier::new(2));
        let release_start = Arc::clone(&start);
        let release = std::thread::spawn(move || {
            release_start.wait();
            let _ = remote::hook_input(&native(false));
            gesture(false);
        });
        start.wait();
        drop(stream);
        release.join().unwrap();
        wait_local();

        assert!(
            remote::hook_input(&native(true)).is_none(),
            "a released side button must not remain quarantined (cycle {cycle})"
        );
        assert!(remote::hook_input(&native(false)).is_none());
        gesture(true);
        gesture(false);
        let deadline = Instant::now() + Duration::from_secs(2);
        while std::fs::metadata(&clicks).map_or(0, |meta| meta.len()) < cycle {
            assert!(
                Instant::now() < deadline,
                "the first local gesture after switching must complete (cycle {cycle})"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            std::fs::metadata(&clicks).unwrap().len(),
            cycle,
            "remote gesture releases must not execute local actions"
        );
    }
    std::fs::remove_dir_all(directory).unwrap();
}
