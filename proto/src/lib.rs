//! hidpipe input wire protocol.
//!
//! The protocol carries raw HID events only: keyboards as USB HID keyboard
//! page (0x07) usage IDs, mice as relative motion and button numbers. It
//! does not understand keymaps or keysyms and does no keymap negotiation;
//! translation happens in static tables local to each end. This is where
//! the system's determinism comes from: there is no state machine on the
//! wire that could drift.
//!
//! Frame format: `[tag: u8][payload]`, multi-byte fields big-endian, over
//! plain TCP for trusted LANs.

#![forbid(unsafe_code)]

use std::io::{self, Read, Write};

use fastrace::collector::{SpanContext, SpanId, TraceId};

#[cfg(unix)]
pub mod bridge;
pub mod clipboard;
pub mod latency;
pub mod pointer;
pub mod trace;
pub use pointer::PointerEvent;

/// Protocol version, carried in the handshake. A mismatch is rejected
/// instead of silently misinterpreting frames.
pub const VERSION: u8 = 2;
/// A client advertising v3 also accepts traced input envelopes.
pub const TRACE_VERSION: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    /// First frame a client sends after connecting.
    Hello {
        version: u8,
        /// Both peers must select the same pointer backend.
        openlogi: bool,
    },
    /// Keyboard event. usage is a USB HID page 0x07 usage ID.
    Key {
        usage: u16,
        state: KeyState,
    },
    /// Relative motion accumulated over one report cycle.
    Motion {
        dx: i16,
        dy: i16,
    },
    Button {
        button: MouseButton,
        state: ButtonState,
    },
    /// Wheel. dy > 0 means the wheel moves away from the user (content
    /// scrolls up). Units are 1/120 of a notch, the same convention as
    /// `PointerEvent::Wheel`, so high-resolution wheels keep sub-notch
    /// precision on the wire.
    Wheel {
        dx: i32,
        dy: i32,
    },
    /// Ask the peer to release all held keys and buttons. Sent before
    /// switching back to local or disconnecting, so no modifiers get stuck
    /// on the peer.
    ReleaseAll,
    Ping,
    Pong,
    /// client to server: the cursor is pinned at the Mac edge facing Linux
    /// and keeps pushing; a request to hand control back. Symmetric to
    /// ReleaseAll: one is the server taking control back, the other is the
    /// user on the client side expressing intent deterministically.
    ReturnToLocal,
    /// Raw OpenLogi pointer input, resolved by the destination's own profile.
    Pointer(PointerEvent),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum KeyState {
    Release = 0,
    Press = 1,
    /// Auto-repeat from holding a key. The peer uses it to set the
    /// autorepeat flag, matching real key repeat behavior.
    Repeat = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum MouseButton {
    Left = 0,
    Right = 1,
    Middle = 2,
    Back = 3,
    Forward = 4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ButtonState {
    Release = 0,
    Press = 1,
}

impl Frame {
    pub fn is_input(&self) -> bool {
        matches!(
            self,
            Self::Key { .. }
                | Self::Motion { .. }
                | Self::Button { .. }
                | Self::Wheel { .. }
                | Self::Pointer(_)
                | Self::ReleaseAll
        )
    }
}

const TAG_HELLO: u8 = 0x00;
const TAG_KEY: u8 = 0x01;
const TAG_MOTION: u8 = 0x02;
const TAG_BUTTON: u8 = 0x03;
const TAG_WHEEL: u8 = 0x04;
const TAG_RELEASE_ALL: u8 = 0x05;
const TAG_PING: u8 = 0x06;
const TAG_PONG: u8 = 0x07;
const TAG_RETURN_TO_LOCAL: u8 = 0x08;
const TAG_POINTER: u8 = 0x09;
const TAG_TRACED_INPUT: u8 = 0x0a;

/// Fixed payload length per tag; None for unknown tags.
pub const fn payload_len(tag: u8) -> Option<usize> {
    match tag {
        TAG_HELLO => Some(2),
        TAG_KEY => Some(3),
        TAG_MOTION => Some(4),
        TAG_BUTTON => Some(2),
        TAG_WHEEL => Some(8),
        TAG_RELEASE_ALL | TAG_PING | TAG_PONG | TAG_RETURN_TO_LOCAL => Some(0),
        TAG_POINTER => Some(pointer::RECORD_SIZE),
        _ => None,
    }
}

pub fn encode(frame: &Frame) -> Vec<u8> {
    let mut out = Vec::with_capacity(5);
    encode_into(frame, &mut out);
    out
}

fn encode_into(frame: &Frame, out: &mut Vec<u8>) {
    match *frame {
        Frame::Hello { version, openlogi } => out.extend([TAG_HELLO, version, u8::from(openlogi)]),
        Frame::Key { usage, state } => {
            out.push(TAG_KEY);
            out.extend(usage.to_be_bytes());
            out.push(state as u8);
        }
        Frame::Motion { dx, dy } => {
            out.push(TAG_MOTION);
            out.extend(dx.to_be_bytes());
            out.extend(dy.to_be_bytes());
        }
        Frame::Button { button, state } => out.extend([TAG_BUTTON, button as u8, state as u8]),
        Frame::Wheel { dx, dy } => {
            out.push(TAG_WHEEL);
            out.extend(dx.to_be_bytes());
            out.extend(dy.to_be_bytes());
        }
        Frame::ReleaseAll => out.push(TAG_RELEASE_ALL),
        Frame::Ping => out.push(TAG_PING),
        Frame::Pong => out.push(TAG_PONG),
        Frame::ReturnToLocal => out.push(TAG_RETURN_TO_LOCAL),
        Frame::Pointer(event) => {
            out.push(TAG_POINTER);
            out.extend(event.encode());
        }
    }
}

pub fn decode(tag: u8, payload: &[u8]) -> Option<Frame> {
    if payload_len(tag)? != payload.len() {
        return None;
    }
    Some(match tag {
        TAG_HELLO => Frame::Hello {
            version: payload[0],
            openlogi: match payload[1] {
                0 => false,
                1 => true,
                _ => return None,
            },
        },
        TAG_KEY => Frame::Key {
            usage: u16::from_be_bytes([payload[0], payload[1]]),
            state: match payload[2] {
                0 => KeyState::Release,
                1 => KeyState::Press,
                _ => KeyState::Repeat,
            },
        },
        TAG_MOTION => Frame::Motion {
            dx: i16::from_be_bytes([payload[0], payload[1]]),
            dy: i16::from_be_bytes([payload[2], payload[3]]),
        },
        TAG_BUTTON => Frame::Button {
            button: match payload[0] {
                0 => MouseButton::Left,
                1 => MouseButton::Right,
                2 => MouseButton::Middle,
                3 => MouseButton::Back,
                4 => MouseButton::Forward,
                _ => return None,
            },
            state: match payload[1] {
                1 => ButtonState::Press,
                _ => ButtonState::Release,
            },
        },
        TAG_WHEEL => Frame::Wheel {
            dx: i32::from_be_bytes(payload[0..4].try_into().ok()?),
            dy: i32::from_be_bytes(payload[4..8].try_into().ok()?),
        },
        TAG_RELEASE_ALL => Frame::ReleaseAll,
        TAG_PING => Frame::Ping,
        TAG_PONG => Frame::Pong,
        TAG_RETURN_TO_LOCAL => Frame::ReturnToLocal,
        TAG_POINTER => Frame::Pointer(PointerEvent::decode(payload.try_into().ok()?).ok()?),
        _ => return None,
    })
}

fn invalid(msg: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

/// Read one frame from the stream. Read timeouts and connection errors
/// surface as Err for the caller to judge.
pub fn read_frame(r: &mut impl Read) -> io::Result<Frame> {
    let mut tag = [0u8; 1];
    r.read_exact(&mut tag)?;
    read_payload(r, tag[0])
}

fn read_payload(r: &mut impl Read, tag: u8) -> io::Result<Frame> {
    let len = payload_len(tag).ok_or(invalid("unknown frame tag"))?;
    let mut payload = [0u8; pointer::RECORD_SIZE];
    r.read_exact(&mut payload[..len])?;
    decode(tag, &payload[..len]).ok_or(invalid("malformed frame"))
}

/// A sampled input carries a 128-bit trace ID and the 64-bit sending span ID.
/// The embedded frame is consumed in this call, including on fragmented TCP reads.
pub fn read_trace_frame(r: &mut impl Read) -> io::Result<(Frame, Option<SpanContext>)> {
    let mut tag = [0];
    r.read_exact(&mut tag)?;
    if tag[0] != TAG_TRACED_INPUT {
        return Ok((read_payload(r, tag[0])?, None));
    }
    let mut trace = [0; 16];
    let mut parent = [0; 8];
    r.read_exact(&mut trace)?;
    r.read_exact(&mut parent)?;
    let trace = TraceId(u128::from_be_bytes(trace));
    let parent = SpanId(u64::from_be_bytes(parent));
    let frame = read_frame(r)?;
    if !frame.is_input() || trace.0 == 0 || parent.0 == 0 {
        return Err(invalid("invalid traced input"));
    }
    Ok((frame, Some(SpanContext::new(trace, parent))))
}

pub fn write_trace_frame(
    w: &mut impl Write,
    frame: &Frame,
    context: SpanContext,
) -> io::Result<()> {
    if !frame.is_input() || !context.sampled || context.trace_id.0 == 0 || context.span_id.0 == 0 {
        return Err(invalid("invalid traced input"));
    }
    let mut out = Vec::with_capacity(26 + pointer::RECORD_SIZE);
    out.push(TAG_TRACED_INPUT);
    out.extend(context.trace_id.0.to_be_bytes());
    out.extend(context.span_id.0.to_be_bytes());
    encode_into(frame, &mut out);
    w.write_all(&out)
}

pub fn write_frame(w: &mut impl Write, frame: &Frame) -> io::Result<()> {
    w.write_all(&encode(frame))
}
