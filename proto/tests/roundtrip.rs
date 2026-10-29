//! Roundtrip consistency for the protocol codec. The wire format is the
//! only cross-machine contract in this system; these tests keep the
//! contract stable across refactors of either end.

use hidpipe_proto::{decode, encode, payload_len, ButtonState, Frame, KeyState, MouseButton};

fn cases() -> Vec<Frame> {
    vec![
        Frame::Hello { version: 0 },
        Frame::Hello { version: 255 },
        Frame::Key {
            usage: 0x04,
            state: KeyState::Press,
        },
        Frame::Key {
            usage: 0xe7,
            state: KeyState::Release,
        },
        Frame::Key {
            usage: 0xffff,
            state: KeyState::Repeat,
        },
        Frame::Motion { dx: 0, dy: 0 },
        Frame::Motion {
            dx: i16::MIN,
            dy: i16::MAX,
        },
        Frame::Button {
            button: MouseButton::Left,
            state: ButtonState::Press,
        },
        Frame::Button {
            button: MouseButton::Right,
            state: ButtonState::Release,
        },
        Frame::Button {
            button: MouseButton::Middle,
            state: ButtonState::Press,
        },
        Frame::Wheel {
            dx: i8::MIN,
            dy: i8::MAX,
        },
        Frame::ReleaseAll,
        Frame::Ping,
        Frame::Pong,
        Frame::ReturnToLocal,
    ]
}

#[test]
fn encode_decode_roundtrip() {
    for frame in cases() {
        let bytes = encode(&frame);
        let (tag, payload) = bytes.split_first().expect("a frame has at least a tag");
        assert_eq!(
            payload_len(*tag).unwrap(),
            payload.len(),
            "payload length does not match tag: {frame:?}"
        );
        assert_eq!(
            decode(*tag, payload),
            Some(frame),
            "roundtrip mismatch: {frame:?}"
        );
    }
}

#[test]
fn rejects_garbage() {
    assert_eq!(payload_len(0xff), None);
    assert_eq!(decode(0x01, &[0x00]), None); // wrong length
    assert_eq!(decode(0xff, &[]), None);
}
