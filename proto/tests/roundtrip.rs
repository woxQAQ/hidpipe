//! Roundtrip consistency for the protocol codec. The wire format is the
//! only cross-machine contract in this system; these tests keep the
//! contract stable across refactors of either end.

use hidpipe_proto::{decode, encode, payload_len, ButtonState, Frame, KeyState, MouseButton};

fn cases() -> Vec<Frame> {
    vec![
        Frame::Hello {
            version: 0,
            openlogi: false,
        },
        Frame::Hello {
            version: 255,
            openlogi: true,
        },
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
            dx: i32::MIN,
            dy: i32::MAX,
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
fn traced_stream_rejects_partial_context_and_control_frames() {
    use fastrace::collector::{SpanContext, SpanId, TraceId};
    use hidpipe_proto::{read_frame, read_trace_frame, write_trace_frame};

    let context = SpanContext::new(TraceId(u128::MAX), SpanId(u64::MAX));
    let mut bytes = Vec::new();
    write_trace_frame(&mut bytes, &Frame::ReleaseAll, context).unwrap();
    assert_eq!(
        read_trace_frame(&mut &bytes[..]).unwrap(),
        (Frame::ReleaseAll, Some(context))
    );
    assert!(read_frame(&mut &bytes[..]).is_err()); // v2 input reader
    for end in 0..bytes.len() {
        assert!(read_trace_frame(&mut &bytes[..end]).is_err());
    }
    for tag in [encode(&Frame::Ping)[0], bytes[0]] {
        let mut malformed = bytes.clone();
        *malformed.last_mut().unwrap() = tag;
        assert!(read_trace_frame(&mut &malformed[..]).is_err());
    }
    bytes[1..17].fill(0);
    assert!(read_trace_frame(&mut &bytes[..]).is_err());
}

#[test]
fn rejects_garbage() {
    assert_eq!(payload_len(0xff), None);
    assert_eq!(decode(0x01, &[0x00]), None); // wrong length
    assert_eq!(decode(0xff, &[]), None);
}
