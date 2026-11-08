//! Native Quartz boundary checks. Events are constructed and read, never posted.
#![cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/diagnostics.rs"]
mod diagnostics;
#[path = "../src/scroll.rs"]
mod scroll;

use core_graphics::event::{CGEventFlags, EventField};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use hidpipe_proto::{Frame, PointerEvent};
use scroll::{Scroll, ScrollModifier};

fn timestamp() -> String {
    "quartz-boundary-test".to_owned()
}

fn modifier(scale: f64) -> ScrollModifier {
    ScrollModifier {
        scale_x: scale,
        scale_y: scale,
        invert_x: false,
        invert_y: false,
    }
}

// Every emitted result is inspected at the real CGEvent boundary.
fn observe(scroll: &mut Scroll, frames: &[Frame]) -> Vec<[i64; 2]> {
    let source = CGEventSource::new(CGEventSourceStateID::Private).unwrap();
    let flags = CGEventFlags::CGEventFlagShift | CGEventFlags::CGEventFlagCommand;
    frames
        .iter()
        .filter_map(|frame| {
            scroll.event(&source, frame, flags).unwrap().map(|event| {
                assert_eq!(
                    event.get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_IS_CONTINUOUS),
                    1
                );
                assert_eq!(event.get_flags(), flags);
                assert_eq!(
                    event.get_integer_value_field(99), // kCGScrollWheelEventScrollPhase
                    0
                );
                assert_eq!(
                    event.get_integer_value_field(123), // kCGScrollWheelEventMomentumPhase
                    0
                );
                [
                    event
                        .get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_POINT_DELTA_AXIS_2),
                    event
                        .get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_POINT_DELTA_AXIS_1),
                ]
            })
        })
        .collect()
}

fn wheel(y: i32) -> Frame {
    Frame::Wheel { dx: 0, dy: y }
}

#[test]
fn quartz_whole_quarters_and_backends() {
    for pointer in [false, true] {
        let frames: Vec<_> = [30, 30, 30, 30]
            .into_iter()
            .map(|dy| {
                if pointer {
                    Frame::Pointer(PointerEvent::Wheel { dx: 0, dy })
                } else {
                    wheel(dy)
                }
            })
            .collect();
        assert_eq!(
            observe(&mut Scroll::new(modifier(1.0)), &frames),
            [[0, 2], [0, 3], [0, 2], [0, 3]]
        );
    }
    assert_eq!(
        observe(&mut Scroll::new(modifier(1.0)), &[wheel(120)]),
        [[0, 10]]
    );
    assert_eq!(
        observe(&mut Scroll::new(modifier(1.0)), &[wheel(1), wheel(119)]),
        [[0, 10]]
    );
    assert_eq!(
        observe(
            &mut Scroll::new(modifier(0.1)),
            &[wheel(30), wheel(30), wheel(30), wheel(30)]
        ),
        [[0, 1]]
    );
}

#[test]
fn quartz_small_partitions_preserve_integer_totals() {
    for scale in [0.1, 0.3, 0.5, 0.7, 1.0, 1.5] {
        for direction in [1, -1] {
            for partition in [
                vec![120],
                vec![1; 120],
                vec![2; 60],
                vec![3; 40],
                vec![7; 17].into_iter().chain([1]).collect(),
                vec![11; 10].into_iter().chain([10]).collect(),
            ] {
                for pointer in [false, true] {
                    for invert in [false, true] {
                        let mut config = modifier(scale);
                        config.invert_x = invert;
                        let frames: Vec<_> = partition
                            .iter()
                            .map(|raw| {
                                let dx = raw * direction;
                                let dy = -dx;
                                if pointer {
                                    Frame::Pointer(PointerEvent::Wheel { dx, dy })
                                } else {
                                    Frame::Wheel { dx, dy }
                                }
                            })
                            .collect();
                        let events = observe(&mut Scroll::new(config), &frames);
                        let total = events
                            .iter()
                            .fold([0, 0], |sum, point| [sum[0] + point[0], sum[1] + point[1]]);
                        let expected = (10.0 * scale) as i64 * i64::from(direction);
                        assert_eq!(
                            total,
                            [if invert { -expected } else { expected }, -expected],
                            "scale={scale}, direction={direction}, reports={}, pointer={pointer}, invert={invert}",
                            frames.len()
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn quartz_boundary_tolerance_preserves_real_deficits_and_tiny_input() {
    // Deficits of 1e-12 and 1e-14 points exceed the rounding allowance near 1.
    // They must survive both batching choices, in both directions.
    for deficit in [1e-12, 1e-14] {
        let scale = 0.1 * (1.0 - deficit);
        for direction in [1, -1] {
            for partition in [vec![120], vec![1; 120]] {
                let mut connection = Scroll::new(modifier(scale));
                let frames: Vec<_> = partition
                    .into_iter()
                    .map(|raw| wheel(raw * direction))
                    .collect();
                assert!(observe(&mut connection, &frames).is_empty());
                assert!(observe(&mut connection, &[wheel(0)]).is_empty());
                assert_eq!(
                    observe(&mut connection, &[wheel(direction)]),
                    [[0, i64::from(direction)]]
                );
            }
        }
    }
    let mut tiny = Scroll::new(modifier(1e-20));
    assert!(observe(&mut tiny, &vec![wheel(1); 120]).is_empty());
    assert!(observe(&mut tiny, &[wheel(120), wheel(-240)]).is_empty());
}

#[test]
fn quartz_axes_direction_and_disabled_scale() {
    let mut config = modifier(0.5);
    config.invert_x = true;
    config.scale_y = 1.5;
    let frames = [Frame::Pointer(PointerEvent::Wheel { dx: 120, dy: -120 })];
    assert_eq!(observe(&mut Scroll::new(config), &frames), [[-5, -15]]);
    assert!(observe(&mut Scroll::new(modifier(0.0)), &frames).is_empty());
    assert_eq!(
        observe(
            &mut Scroll::new(modifier(1.0)),
            &[wheel(-30), wheel(-30), wheel(-30), wheel(-30)]
        ),
        [[0, -2], [0, -3], [0, -2], [0, -3]]
    );
    assert_eq!(
        observe(
            &mut Scroll::new(modifier(1.0)),
            &[wheel(1), wheel(-1), wheel(12)]
        ),
        [[0, 1]]
    );
    assert_eq!(
        observe(&mut Scroll::new(modifier(1.0)), &[wheel(30), wheel(-30)]),
        [[0, 2], [0, -2]]
    );
    let frames = [
        Frame::Pointer(PointerEvent::Wheel { dx: 6, dy: -3 }),
        Frame::Pointer(PointerEvent::Wheel { dx: 6, dy: -9 }),
    ];
    assert_eq!(observe(&mut Scroll::new(modifier(1.0)), &frames), [[1, -1]]);
}

#[test]
fn quartz_reset_and_new_connection_discard_remainders() {
    for reset in [Frame::ReleaseAll, Frame::Pointer(PointerEvent::Reset)] {
        let mut connection = Scroll::new(modifier(0.1));
        assert!(observe(&mut connection, &vec![wheel(1); 119]).is_empty());
        assert!(observe(&mut connection, &[reset, wheel(1)]).is_empty());
        assert_eq!(observe(&mut connection, &[wheel(119)]), [[0, 1]]);
        assert_eq!(
            observe(
                &mut Scroll::new(modifier(1.0)),
                &[wheel(6), reset, wheel(6), wheel(6)]
            ),
            [[0, 1]]
        );
    }
    {
        let mut connection = Scroll::new(modifier(1.0));
        assert!(observe(&mut connection, &[wheel(6)]).is_empty());
    }
    assert!(observe(&mut Scroll::new(modifier(1.0)), &[wheel(6)]).is_empty());
}

#[test]
fn quartz_overflow_saturates_without_poisoning_later_events() {
    let mut connection = Scroll::new(modifier(f64::MAX));
    assert_eq!(
        observe(
            &mut connection,
            &[
                Frame::Pointer(PointerEvent::Wheel {
                    dx: i32::MAX,
                    dy: i32::MIN
                }),
                wheel(0),
                wheel(1),
                wheel(-1),
            ]
        ),
        [
            [i64::from(i32::MAX), i64::from(i32::MIN)],
            [0, i64::from(i32::MAX)],
            [0, i64::from(i32::MIN)]
        ]
    );
}
