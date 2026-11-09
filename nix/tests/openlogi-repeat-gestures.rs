//! Verify repeated gestures through local capture and destination socket routing.
#![cfg(target_os = "linux")]
#![expect(clippy::unwrap_used, reason = "integration test assertions")]

use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use openlogi_agent_core::{orchestrator::Orchestrator, remote};
use openlogi_core::binding::{Action, ButtonId, GESTURE_HOLD_FOR_SWIPE, GestureDirection};
use openlogi_core::config::Config;
use openlogi_hid::gesture::CapturedInput;
use remote::wire::{MAGIC, PointerEvent};

fn button(pressed: bool) -> PointerEvent {
    PointerEvent::Button { button: 5, pressed }
}

fn wait_actions(path: &Path, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let actual = std::fs::read_to_string(path).unwrap_or_default();
        if actual == expected {
            return;
        }
        assert!(
            expected.starts_with(&actual) && Instant::now() < deadline,
            "expected actions {expected:?}, got {actual:?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn swipe(send: &mut impl FnMut(PointerEvent), dx: i32, dy: i32) {
    std::thread::sleep(GESTURE_HOLD_FOR_SWIPE + Duration::from_millis(20));
    send(PointerEvent::GestureMotion { dx, dy });
}

fn exercise(mut send: impl FnMut(PointerEvent), actions: &Path, expected: &mut String) {
    send(button(true));
    swipe(&mut send, 60, 0);
    expected.push('R');
    wait_actions(actions, expected);

    // A single hold supports repeated directions and then a reversal.
    swipe(&mut send, 60, 0);
    expected.push('R');
    wait_actions(actions, expected);
    swipe(&mut send, -60, 0);
    expected.push('L');
    wait_actions(actions, expected);

    // Both axes must start from zero after a commit.
    swipe(&mut send, 0, -60);
    expected.push('U');
    wait_actions(actions, expected);
    send(button(false));
    send(PointerEvent::GestureMotion { dx: 100, dy: 0 });
    std::thread::sleep(GESTURE_HOLD_FOR_SWIPE + Duration::from_millis(20));
    assert_eq!(std::fs::read_to_string(actions).unwrap(), *expected);

    // A fresh press with only tiny drift still executes Click.
    send(button(true));
    send(PointerEvent::GestureMotion { dx: 2, dy: -1 });
    send(button(false));
    expected.push('C');
    wait_actions(actions, expected);

    // Cancellation after a swipe must clear the hold without clicking.
    send(button(true));
    swipe(&mut send, -60, 0);
    expected.push('L');
    wait_actions(actions, expected);
    send(PointerEvent::Reset);
    swipe(&mut send, 100, 0);
    send(button(false));
    send(button(true));
    send(button(false));
    expected.push('C');
    wait_actions(actions, expected);
}

#[test]
fn held_gestures_repeat_locally_and_at_the_destination() {
    let directory =
        std::env::temp_dir().join(format!("openlogi-repeat-gestures-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = directory.join("pointer.sock");
    let actions = directory.join("actions");
    let mut config = Config::default();
    for (direction, marker) in [
        (GestureDirection::Right, 'R'),
        (GestureDirection::Left, 'L'),
        (GestureDirection::Up, 'U'),
        (GestureDirection::Click, 'C'),
    ] {
        config.set_gesture_direction(
            "mouse",
            ButtonId::GestureButton,
            direction,
            Action::RunShellCommand(format!("printf {marker} >> '{}'", actions.display())),
        );
    }
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
    let mut expected = String::new();
    exercise(
        |event| {
            let input = match event {
                PointerEvent::Button { pressed, .. } => CapturedInput::RawButton {
                    button: ButtonId::GestureButton,
                    pressed,
                },
                PointerEvent::GestureMotion { dx, dy } => CapturedInput::RawMotion {
                    dx: i16::try_from(dx).unwrap(),
                    dy: i16::try_from(dy).unwrap(),
                },
                PointerEvent::Reset => CapturedInput::Interrupted,
                _ => unreachable!(),
            };
            assert!(remote::captured("mouse", input));
        },
        &actions,
        &mut expected,
    );

    let mut resolver = UnixStream::connect(socket).unwrap();
    resolver
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    resolver.write_all(&MAGIC).unwrap();
    resolver.write_all(&[1]).unwrap();
    let mut ack = [1];
    resolver.read_exact(&mut ack).unwrap();
    assert_eq!(ack, [0]);
    exercise(
        |event| {
            event.write(&mut resolver).unwrap();
            resolver.read_exact(&mut ack).unwrap();
            assert_eq!(ack, [1], "gesture input must be consumed");
        },
        &actions,
        &mut expected,
    );
    std::fs::remove_dir_all(directory).unwrap();
}
