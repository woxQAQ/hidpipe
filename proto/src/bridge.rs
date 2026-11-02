//! Local socket clients for the OpenLogi pointer bridge.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::pointer::{PointerEvent, MAGIC, RECORD_SIZE};

const TIMEOUT: Duration = Duration::from_millis(500);

fn connect(path: &Path, role: u8) -> io::Result<UnixStream> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    stream.write_all(&MAGIC)?;
    stream.write_all(&[role])?;
    let mut ack = [0];
    stream.read_exact(&mut ack)?;
    if ack != [0] {
        return Err(io::Error::other(
            "OpenLogi refused the pointer bridge lease",
        ));
    }
    Ok(stream)
}

/// A capture lease. Dropping the socket restores source-side routing.
pub struct Capture {
    stream: UnixStream,
    bytes: [u8; RECORD_SIZE],
    filled: usize,
    heartbeat: Instant,
}

impl Capture {
    /// Acquire capture only once the destination and keyboard are ready.
    pub fn connect(path: &Path) -> io::Result<Self> {
        let stream = connect(path, 0)?;
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream,
            bytes: [0; RECORD_SIZE],
            filled: 0,
            heartbeat: Instant::now(),
        })
    }

    /// Poll one event, retaining partial records across nonblocking reads.
    pub fn poll(&mut self) -> io::Result<Option<PointerEvent>> {
        if self.heartbeat.elapsed() >= Duration::from_millis(200) {
            self.stream.write_all(&[0])?;
            self.heartbeat = Instant::now();
        }
        loop {
            match self.stream.read(&mut self.bytes[self.filled..]) {
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(n) => self.filled += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(None),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
            if self.filled == RECORD_SIZE {
                self.filled = 0;
                return PointerEvent::decode(self.bytes).map(Some);
            }
        }
    }
}

/// Resolves inputs using the destination agent's own OpenLogi profile.
pub struct Resolver(UnixStream);

impl Resolver {
    /// Connect before announcing the network client as ready.
    pub fn connect(path: &Path) -> io::Result<Self> {
        connect(path, 1).map(Self)
    }

    /// Returns whether the ordinary input should be injected by hidpipe.
    /// Any socket failure aborts the network session, releasing held input.
    pub fn resolve(&mut self, event: PointerEvent) -> io::Result<bool> {
        event.write(&mut self.0)?;
        let mut ack = [0];
        self.0.read_exact(&mut ack)?;
        match ack[0] {
            0 => Ok(true),
            1 => Ok(false),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid OpenLogi disposition",
            )),
        }
    }
}
