//! Optional text and image clipboard channel, independent of the HID stream.
//!
//! Both peers baseline their local clipboard on connection. Only subsequent
//! changes are shared, so reconnecting does not overwrite an existing clipboard.

use std::io::{self, Read, Write};
#[cfg(target_os = "linux")]
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
#[cfg(target_os = "linux")]
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::time::{Duration, Instant};
use std::{net, thread};

const MAGIC: &[u8; 8] = b"HIDCLIP\x02";
pub const MAX_TEXT_BYTES: usize = 1024 * 1024;
pub const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;

/// PNG is the shared image representation on Wayland and macOS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    Text(String),
    Png(Vec<u8>),
}

impl Content {
    fn payload(&self) -> &[u8] {
        match self {
            Self::Text(text) => text.as_bytes(),
            Self::Png(bytes) => bytes,
        }
    }

    fn kind(&self) -> u8 {
        match self {
            Self::Text(_) => 0,
            Self::Png(_) => 1,
        }
    }

    fn limit(&self) -> usize {
        match self {
            Self::Text(_) => MAX_TEXT_BYTES,
            Self::Png(_) => MAX_IMAGE_BYTES,
        }
    }
}

const POLL: Duration = Duration::from_millis(250);
const IO_TIMEOUT: Duration = Duration::from_secs(2);
const HEARTBEAT: Duration = Duration::from_secs(2);
const DEAD_TIMEOUT: Duration = Duration::from_secs(8);
const PING: u32 = u32::MAX;

/// A missing supported representation differs from an explicitly empty string.
pub trait Clipboard {
    fn read(&mut self) -> io::Result<Option<Content>>;
    fn write(&mut self, content: &Content) -> io::Result<()>;
}

pub struct SystemClipboard;

#[cfg(target_os = "linux")]
impl Clipboard for SystemClipboard {
    fn read(&mut self) -> io::Result<Option<Content>> {
        let (success, types) = execute(Command::new("wl-paste").arg("--list-types"), None)?;
        if !success {
            return Ok(None);
        }
        let image = types
            .split(|byte| *byte == b'\n')
            .any(|mime| mime == b"image/png");
        let (success, output) = execute(
            Command::new("wl-paste").args([
                "--no-newline",
                "--type",
                if image { "image/png" } else { "text" },
            ]),
            None,
        )?;
        if !success {
            return Ok(None);
        }
        if image {
            Ok(Some(Content::Png(output)))
        } else {
            if output.len() > MAX_TEXT_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "clipboard text exceeds 1 MiB",
                ));
            }
            let text = String::from_utf8(output).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "clipboard is not UTF-8")
            })?;
            Ok(Some(Content::Text(text)))
        }
    }

    fn write(&mut self, content: &Content) -> io::Result<()> {
        let mime = match content {
            Content::Text(_) => "text/plain;charset=utf-8",
            Content::Png(_) => "image/png",
        };
        if execute(
            Command::new("wl-copy").args(["--type", mime]),
            Some(content.payload()),
        )?
        .0
        {
            Ok(())
        } else {
            Err(io::Error::other("clipboard command failed"))
        }
    }
}

/// Bound command execution and output, including clipboard owners that stop responding.
#[cfg(target_os = "linux")]
fn execute(command: &mut Command, input: Option<&[u8]>) -> io::Result<(bool, Vec<u8>)> {
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(if input.is_some() {
            Stdio::null()
        } else {
            Stdio::piped()
        })
        .stderr(Stdio::null());
    // Pasteboard tools use the process locale to select their text encoding.
    command.env("LC_CTYPE", "en_US.UTF-8");
    let mut child = command.spawn()?;
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let input = input.map(Vec::from);
    let transfer = thread::spawn(move || -> io::Result<Vec<u8>> {
        if let (Some(mut stdin), Some(input)) = (stdin, input) {
            stdin.write_all(&input)?;
        }
        let mut output = Vec::new();
        if let Some(stdout) = stdout {
            stdout
                .take((MAX_IMAGE_BYTES + 1) as u64)
                .read_to_end(&mut output)?;
        }
        if output.len() > MAX_IMAGE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "clipboard exceeds 16 MiB",
            ));
        }
        Ok(output)
    });
    let deadline = Instant::now() + IO_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status.success()),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(error);
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "clipboard command timed out",
                ));
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
        }
    };
    let output = transfer
        .join()
        .map_err(|_| io::Error::other("clipboard worker panicked"))?;
    Ok((status?, output?))
}

#[cfg(target_os = "linux")]
pub fn spawn_server(bind: &str, port: u16) -> io::Result<()> {
    let listener = TcpListener::bind((bind, port))?;
    thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    if let Err(error) = session(stream, &mut SystemClipboard) {
                        eprintln!("[clipboard] connection ended: {error}");
                    }
                }
                Err(error) => eprintln!("[clipboard] accept failed: {error}"),
            }
        }
    });
    Ok(())
}

pub fn spawn_client(host: String, port: u16, mut clipboard: impl Clipboard + Send + 'static) {
    thread::spawn(move || loop {
        let result = (host.as_str(), port)
            .to_socket_addrs()
            .and_then(|addresses| {
                let mut last = io::Error::other("clipboard host has no addresses");
                for address in addresses {
                    match TcpStream::connect_timeout(&address, IO_TIMEOUT) {
                        Ok(stream) => return session(stream, &mut clipboard),
                        Err(error) => last = error,
                    }
                }
                Err(last)
            });
        if let Err(error) = result {
            eprintln!("[clipboard] connection ended: {error}");
        }
        thread::sleep(Duration::from_secs(2));
    });
}

/// Run a bidirectional session. Partial reads retain their bytes across timeouts.
/// Records contain a type byte followed by bounded UTF-8 text or PNG bytes.
pub fn session(mut stream: TcpStream, clipboard: &mut impl Clipboard) -> io::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    stream.write_all(MAGIC)?;
    let mut magic = [0; 8];
    stream.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "clipboard protocol mismatch",
        ));
    }
    stream.set_read_timeout(Some(Duration::from_millis(50)))?;
    eprintln!("[clipboard] connected");
    let outgoing = Outgoing::new(&stream)?;
    let initial = clipboard.read();
    let mut initialized = initial.is_ok();
    let mut previous = initial.ok().flatten();
    let mut incoming = Vec::new();
    let mut buffer = [0; 8192];
    let mut next_poll = Instant::now() + POLL;
    let mut last_received = Instant::now();
    let mut last_sent = Instant::now();
    let mut backend_failed = false;
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(size) => {
                last_received = Instant::now();
                incoming.extend_from_slice(&buffer[..size]);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(error),
        }
        while incoming.len() >= 4 {
            let size = u32::from_be_bytes(incoming[..4].try_into().unwrap());
            if size == PING {
                incoming.drain(..4);
                continue;
            }
            if size == 0 || size as usize > MAX_IMAGE_BYTES + 1 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid clipboard record size",
                ));
            }
            if incoming.len() < 5 {
                break;
            }
            let limit = match incoming[4] {
                0 => MAX_TEXT_BYTES,
                1 => MAX_IMAGE_BYTES,
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "unknown clipboard type",
                    ))
                }
            };
            if size as usize > limit + 1 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "clipboard payload exceeds limit",
                ));
            }
            let end = 4 + size as usize;
            if incoming.len() < end {
                break;
            }
            let payload = &incoming[5..end];
            let content = match incoming[4] {
                0 => Content::Text(String::from_utf8(payload.to_vec()).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "clipboard is not UTF-8")
                })?),
                _ => {
                    if !payload.starts_with(b"\x89PNG\r\n\x1a\n") {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "clipboard image is not PNG",
                        ));
                    }
                    Content::Png(payload.to_vec())
                }
            };
            if previous.as_ref() != Some(&content) {
                match clipboard.write(&content) {
                    Ok(()) => {
                        previous = Some(content);
                        initialized = true;
                        backend_failed = false;
                    }
                    Err(error) => report_backend_error(&mut backend_failed, error),
                }
            }
            incoming.drain(..end);
        }
        if Instant::now() >= next_poll {
            match clipboard.read() {
                Ok(current) => {
                    backend_failed = false;
                    if !initialized {
                        previous = current;
                        initialized = true;
                    } else if current != previous {
                        let mut accepted = true;
                        if let Some(content) = &current {
                            let payload = content.payload();
                            if payload.len() <= content.limit() {
                                let mut record = Vec::with_capacity(5 + payload.len());
                                record
                                    .extend_from_slice(&((payload.len() + 1) as u32).to_be_bytes());
                                record.push(content.kind());
                                record.extend_from_slice(payload);
                                accepted = outgoing.send(record)?;
                                if accepted {
                                    last_sent = Instant::now();
                                }
                            }
                        }
                        if accepted {
                            previous = current;
                        }
                    }
                }
                Err(error) => report_backend_error(&mut backend_failed, error),
            }
            next_poll = Instant::now() + POLL;
        }
        if last_sent.elapsed() >= HEARTBEAT && outgoing.send(PING.to_be_bytes().to_vec())? {
            last_sent = Instant::now();
        }
        if last_received.elapsed() >= DEAD_TIMEOUT {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "clipboard peer timed out",
            ));
        }
    }
}

/// A bounded writer lets both peers receive while sending a large clipboard.
/// Dropping the session shuts down and joins the writer before reconnecting.
struct Outgoing {
    tx: Option<SyncSender<Vec<u8>>>,
    stream: TcpStream,
    worker: Option<thread::JoinHandle<()>>,
}

impl Outgoing {
    fn new(stream: &TcpStream) -> io::Result<Self> {
        let mut writer = stream.try_clone()?;
        let stream = stream.try_clone()?;
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(1);
        let worker = thread::spawn(move || {
            while let Ok(record) = rx.recv() {
                if writer.write_all(&record).is_err() {
                    let _ = writer.shutdown(net::Shutdown::Both);
                    break;
                }
            }
        });
        Ok(Self {
            tx: Some(tx),
            stream,
            worker: Some(worker),
        })
    }

    fn send(&self, record: Vec<u8>) -> io::Result<bool> {
        match self.tx.as_ref().unwrap().try_send(record) {
            Ok(()) => Ok(true),
            Err(TrySendError::Full(_)) => Ok(false),
            Err(TrySendError::Disconnected(_)) => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "clipboard writer stopped",
            )),
        }
    }
}

impl Drop for Outgoing {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(net::Shutdown::Both);
        self.tx.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn report_backend_error(failed: &mut bool, error: io::Error) {
    if !*failed {
        eprintln!("[clipboard] local clipboard unavailable: {error}");
    }
    *failed = true;
}
