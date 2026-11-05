//! Optional text clipboard channel, independent of the latency-sensitive HID stream.
//!
//! Both peers baseline their local clipboard on connection. Only subsequent
//! changes are shared, so reconnecting does not overwrite an existing clipboard.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::thread;
use std::time::{Duration, Instant};

const MAGIC: &[u8; 8] = b"HIDCLIP\x01";
pub const MAX_TEXT_BYTES: usize = 1024 * 1024;
const POLL: Duration = Duration::from_millis(250);
const IO_TIMEOUT: Duration = Duration::from_secs(2);
const HEARTBEAT: Duration = Duration::from_secs(2);
const DEAD_TIMEOUT: Duration = Duration::from_secs(8);
const PING: u32 = u32::MAX;

/// A missing text representation is distinct from an explicitly empty string.
pub trait Clipboard {
    fn read(&mut self) -> io::Result<Option<String>>;
    fn write(&mut self, text: &str) -> io::Result<()>;
}

pub struct SystemClipboard;

impl Clipboard for SystemClipboard {
    fn read(&mut self) -> io::Result<Option<String>> {
        #[cfg(target_os = "linux")]
        let mut command = {
            let mut command = Command::new("wl-paste");
            command.args(["--no-newline", "--type", "text"]);
            command
        };
        #[cfg(target_os = "macos")]
        let mut command = Command::new("/usr/bin/pbpaste");
        let (success, output) = execute(&mut command, None)?;
        if !success {
            return Ok(None);
        }
        let text = String::from_utf8(output)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "clipboard is not UTF-8"))?;
        Ok(Some(text))
    }

    fn write(&mut self, text: &str) -> io::Result<()> {
        #[cfg(target_os = "linux")]
        let mut command = {
            let mut command = Command::new("wl-copy");
            command.args(["--type", "text/plain;charset=utf-8"]);
            command
        };
        #[cfg(target_os = "macos")]
        let mut command = Command::new("/usr/bin/pbcopy");
        if execute(&mut command, Some(text.as_bytes()))?.0 {
            Ok(())
        } else {
            Err(io::Error::other("clipboard command failed"))
        }
    }
}

/// Bound command execution and output, including clipboard owners that stop responding.
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
                .take((MAX_TEXT_BYTES + 1) as u64)
                .read_to_end(&mut output)?;
        }
        if output.len() > MAX_TEXT_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "clipboard exceeds 1 MiB",
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

pub fn spawn_client(host: String, port: u16) {
    thread::spawn(move || loop {
        let result = (host.as_str(), port)
            .to_socket_addrs()
            .and_then(|addresses| {
                let mut last = io::Error::other("clipboard host has no addresses");
                for address in addresses {
                    match TcpStream::connect_timeout(&address, IO_TIMEOUT) {
                        Ok(stream) => return session(stream, &mut SystemClipboard),
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
/// The only variable-size records are bounded UTF-8 text; PING has no payload.
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
            if size as usize > MAX_TEXT_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "clipboard exceeds 1 MiB",
                ));
            }
            let end = 4 + size as usize;
            if incoming.len() < end {
                break;
            }
            let text = std::str::from_utf8(&incoming[4..end]).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "clipboard is not UTF-8")
            })?;
            if previous.as_deref() != Some(text) {
                match clipboard.write(text) {
                    Ok(()) => {
                        previous = Some(text.to_owned());
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
                        if let Some(text) = &current {
                            if text.len() <= MAX_TEXT_BYTES {
                                let mut record = Vec::with_capacity(4 + text.len());
                                record.extend_from_slice(&(text.len() as u32).to_be_bytes());
                                record.extend_from_slice(text.as_bytes());
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
                    let _ = writer.shutdown(std::net::Shutdown::Both);
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
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
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
