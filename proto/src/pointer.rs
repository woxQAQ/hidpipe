//! OpenLogi pointer bridge protocol, version 1.
//!
//! The local socket starts with `OLHP`, version 1, then role 0 (capture)
//! or 1 (resolve). The agent replies with 0 on success. Capture owns a lease
//! until EOF; resolve replies to every record with 0 (pass) or 1 (consume).
//! Records are 12 bytes: tag, zero padding, and two big-endian i32 values.
//! Wheel values use 120 units per notch. Button IDs 0..4 are left/right/
//! middle/back/forward, 5 is the gesture control, 6 DPI, 7 thumb-wheel tap.

use std::io::{self, Read, Write};

/// Fixed size of one pointer record, including its tag.
pub const RECORD_SIZE: usize = 12;
/// Local bridge handshake prefix and version.
pub const MAGIC: [u8; 5] = *b"OLHP\x01";

/// Input before any platform-specific binding is resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerEvent {
    /// Relative pointer movement.
    Motion { dx: i32, dy: i32 },
    /// Button state, using the IDs defined by the bridge protocol.
    Button { button: u8, pressed: bool },
    /// Horizontal and vertical wheel movement, in 1/120-notch units.
    Wheel { dx: i32, dy: i32 },
    /// Dedicated gesture control movement; this does not move the pointer.
    GestureMotion { dx: i32, dy: i32 },
    /// Diverted thumb-wheel rotation in native device increments.
    ThumbWheel(i32),
    /// Cancel pending gestures and wheel accumulators without firing actions.
    Reset,
}

impl PointerEvent {
    /// Encode one fixed-size record.
    #[must_use]
    pub fn encode(self) -> [u8; RECORD_SIZE] {
        let (tag, a, b) = match self {
            Self::Motion { dx, dy } => (0, dx, dy),
            Self::Button { button, pressed } => (1, i32::from(button), i32::from(pressed)),
            Self::Wheel { dx, dy } => (2, dx, dy),
            Self::GestureMotion { dx, dy } => (3, dx, dy),
            Self::ThumbWheel(delta) => (4, delta, 0),
            Self::Reset => (5, 0, 0),
        };
        let mut bytes = [0; RECORD_SIZE];
        bytes[0] = tag;
        bytes[4..8].copy_from_slice(&a.to_be_bytes());
        bytes[8..12].copy_from_slice(&b.to_be_bytes());
        bytes
    }

    /// Decode a record, rejecting unknown values and nonzero reserved fields.
    pub fn decode(bytes: [u8; RECORD_SIZE]) -> io::Result<Self> {
        let a = i32::from_be_bytes(bytes[4..8].try_into().map_err(|_| invalid())?);
        let b = i32::from_be_bytes(bytes[8..12].try_into().map_err(|_| invalid())?);
        if bytes[1..4] != [0; 3] {
            return Err(invalid());
        }
        match (bytes[0], a, b) {
            (0, dx, dy) => Ok(Self::Motion { dx, dy }),
            (1, button @ 0..=7, pressed @ 0..=1) => Ok(Self::Button {
                button: u8::try_from(button).map_err(|_| invalid())?,
                pressed: pressed == 1,
            }),
            (2, dx, dy) => Ok(Self::Wheel { dx, dy }),
            (3, dx, dy) => Ok(Self::GestureMotion { dx, dy }),
            (4, delta, 0) => Ok(Self::ThumbWheel(delta)),
            (5, 0, 0) => Ok(Self::Reset),
            _ => Err(invalid()),
        }
    }

    /// Read a complete record. EOF or a partial record terminates the lease.
    pub fn read(reader: &mut impl Read) -> io::Result<Self> {
        let mut bytes = [0; RECORD_SIZE];
        reader.read_exact(&mut bytes)?;
        Self::decode(bytes)
    }

    /// Write a complete record.
    pub fn write(self, writer: &mut impl Write) -> io::Result<()> {
        writer.write_all(&self.encode())
    }
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid OpenLogi pointer record",
    )
}
