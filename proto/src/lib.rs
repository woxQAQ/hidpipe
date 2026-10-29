//! hidpipe wire protocol v0.
//!
//! The protocol carries raw HID events only: keyboards as USB HID keyboard
//! page (0x07) usage IDs, mice as relative motion and button numbers. It
//! does not understand keymaps or keysyms and does no keymap negotiation;
//! translation happens in static tables local to each end. This is where
//! the system's determinism comes from: there is no state machine on the
//! wire that could drift.
//!
//! Frame format: `[tag: u8][payload]`, multi-byte fields big-endian. v0 is
//! plain TCP for trusted LANs only.

#![forbid(unsafe_code)]

use std::io::{self, Read, Write};

/// Protocol version, carried in the handshake. A mismatch is rejected
/// instead of silently misinterpreting frames.
pub const VERSION: u8 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    /// First frame a client sends after connecting.
    Hello {
        version: u8,
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
    /// scrolls up).
    Wheel {
        dx: i8,
        dy: i8,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ButtonState {
    Release = 0,
    Press = 1,
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

/// Fixed payload length per tag; None for unknown tags.
pub const fn payload_len(tag: u8) -> Option<usize> {
    match tag {
        TAG_HELLO => Some(1),
        TAG_KEY => Some(3),
        TAG_MOTION => Some(4),
        TAG_BUTTON | TAG_WHEEL => Some(2),
        TAG_RELEASE_ALL | TAG_PING | TAG_PONG | TAG_RETURN_TO_LOCAL => Some(0),
        _ => None,
    }
}

pub fn encode(frame: &Frame) -> Vec<u8> {
    let mut out = Vec::with_capacity(5);
    match *frame {
        Frame::Hello { version } => out.extend([TAG_HELLO, version]),
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
        Frame::Wheel { dx, dy } => out.extend([TAG_WHEEL, dx as u8, dy as u8]),
        Frame::ReleaseAll => out.push(TAG_RELEASE_ALL),
        Frame::Ping => out.push(TAG_PING),
        Frame::Pong => out.push(TAG_PONG),
        Frame::ReturnToLocal => out.push(TAG_RETURN_TO_LOCAL),
    }
    out
}

pub fn decode(tag: u8, payload: &[u8]) -> Option<Frame> {
    if payload_len(tag)? != payload.len() {
        return None;
    }
    Some(match tag {
        TAG_HELLO => Frame::Hello {
            version: payload[0],
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
                _ => MouseButton::Middle,
            },
            state: match payload[1] {
                1 => ButtonState::Press,
                _ => ButtonState::Release,
            },
        },
        TAG_WHEEL => Frame::Wheel {
            dx: payload[0] as i8,
            dy: payload[1] as i8,
        },
        TAG_RELEASE_ALL => Frame::ReleaseAll,
        TAG_PING => Frame::Ping,
        TAG_PONG => Frame::Pong,
        TAG_RETURN_TO_LOCAL => Frame::ReturnToLocal,
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
    let len = payload_len(tag[0]).ok_or(invalid("unknown frame tag"))?;
    let mut payload = [0u8; 8];
    r.read_exact(&mut payload[..len])?;
    decode(tag[0], &payload[..len]).ok_or(invalid("malformed frame"))
}

pub fn write_frame(w: &mut impl Write, frame: &Frame) -> io::Result<()> {
    w.write_all(&encode(frame))
}
