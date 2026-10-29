//! 协议编解码的往返一致性:线上格式是本系统唯一的跨机契约,
//! 任何一端重构实现后,这组测试保证契约不变。

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
        let (tag, payload) = bytes.split_first().expect("帧至少含 tag");
        assert_eq!(
            payload_len(*tag).unwrap(),
            payload.len(),
            "长度与 tag 不符: {frame:?}"
        );
        assert_eq!(decode(*tag, payload), Some(frame), "往返不一致: {frame:?}");
    }
}

#[test]
fn rejects_garbage() {
    assert_eq!(payload_len(0xff), None);
    assert_eq!(decode(0x01, &[0x00]), None); // 长度不符
    assert_eq!(decode(0xff, &[]), None);
}
