//! Read/write threads for a single client TCP connection.
//!
//! Keepalive: the writer sends a Ping every 2s; the reader declares the
//! connection dead after 6s without any frame and reports
//! `NetMsg::Dead(connection generation)`. The main loop uses the generation
//! to ignore late death notices from superseded connections.

use std::net::TcpStream;
use std::num::NonZeroU32;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};
use std::{io, net};

use fastrace::{collector::SpanContext, Span};
use hidpipe_proto as proto;
use hidpipe_proto::latency::LatencyStats;
use proto::Frame;

const PING_INTERVAL: Duration = Duration::from_secs(2);
const READ_TIMEOUT: Duration = Duration::from_secs(6);

pub enum NetMsg {
    /// The pending connection's first frame was a version-matching Hello;
    /// it may take over.
    Hello {
        id: u64,
        openlogi: bool,
        supports_trace: bool,
    },
    /// The client asks to hand control back (cursor pushed against the
    /// Mac's left edge).
    ReturnToLocal(u64),
    /// The connection is dead (read timeout, write failure, peer closed).
    /// Carries the connection generation.
    Dead(u64),
    /// The connection never became the serving one: its first frame was not
    /// a version-matching Hello. Admission control: a peer that cannot
    /// handshake must not displace a healthy connection.
    Rejected(u64, Rejection),
}

/// Why a pending connection failed admission. Judged from the first frame
/// only, so a stray or outdated peer on the port cannot displace a serving
/// connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    /// The peer closed before sending anything.
    Closed,
    /// Nothing arrived within the read timeout.
    Silent,
    /// A frame that was not a Hello.
    Unexpected,
    /// A Hello from another protocol version.
    Version(u8),
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Closed => write!(f, "closed before the hello"),
            Self::Silent => write!(f, "no hello within {READ_TIMEOUT:?}"),
            Self::Unexpected => write!(f, "first frame was not a hello"),
            Self::Version(version) => write!(
                f,
                "unsupported input protocol version {version} (supported: {}, {})",
                proto::VERSION,
                proto::TRACE_VERSION
            ),
        }
    }
}

struct QueuedFrame {
    frame: Frame,
    queued_at: Option<Instant>,
    trace: Option<Box<ForwardTrace>>,
}

struct ForwardTrace {
    forward: Span,
    queue: Span,
}

pub struct Connection {
    pub id: u64,
    tx: Sender<QueuedFrame>,
    latency_stats: bool,
    pub trace_every: Option<NonZeroU32>,
    until_sample: u32,
    shutdown: TcpStream,
}

impl Connection {
    pub fn spawn(
        stream: TcpStream,
        net_tx: Sender<NetMsg>,
        id: u64,
        latency_stats: bool,
    ) -> io::Result<Self> {
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(READ_TIMEOUT))?;
        let reader = stream.try_clone()?;
        let shutdown = stream.try_clone()?;
        let (tx, rx) = mpsc::channel::<QueuedFrame>();
        thread::spawn({
            let net_tx = net_tx.clone();
            move || writer_loop(stream, rx, net_tx, id, latency_stats)
        });
        thread::spawn({
            let tx = tx.clone();
            move || reader_loop(reader, tx, net_tx, id)
        });
        Ok(Self {
            id,
            tx,
            latency_stats,
            trace_every: None,
            until_sample: 0,
            shutdown,
        })
    }

    /// A failed send means the peer is dead; the read/write threads report
    /// Dead shortly after, so the frame is dropped silently here.
    pub fn send(&mut self, frame: Frame) {
        let trace = self
            .trace_every
            .filter(|_| frame.is_input())
            .and_then(|every| {
                if self.until_sample > 0 {
                    self.until_sample -= 1;
                    return None;
                }
                self.until_sample = every.get() - 1;
                let forward = Span::root("server.forward", SpanContext::random());
                let queue = Span::enter_with_parent("server.queue", &forward);
                Some(Box::new(ForwardTrace { forward, queue }))
            });
        let _ = self.tx.send(QueuedFrame {
            trace,
            frame,
            queued_at: self.latency_stats.then(Instant::now),
        });
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.shutdown.shutdown(net::Shutdown::Both);
    }
}

fn writer_loop(
    mut stream: TcpStream,
    rx: Receiver<QueuedFrame>,
    net_tx: Sender<NetMsg>,
    id: u64,
    latency_stats: bool,
) {
    let mut stats = latency_stats.then(|| LatencyStats::new(["server.queue", "server.write"]));
    // Pings must go out on schedule, independent of data traffic. If we
    // only pinged after 2s of channel idle, constant mouse motion would
    // starve Ping forever: the peer would never see a Ping, never answer
    // Pong, and our reader would wrongly declare the connection dead after
    // 6s.
    let mut next_ping = Instant::now() + PING_INTERVAL;
    loop {
        let (frame, trace) =
            match rx.recv_timeout(next_ping.saturating_duration_since(Instant::now())) {
                Ok(queued) => {
                    if let (Some(stats), Some(started)) = (stats.as_mut(), queued.queued_at) {
                        stats.record(0, started.elapsed());
                    }
                    let forward = queued.trace.map(|trace| {
                        drop(trace.queue);
                        trace.forward
                    });
                    (queued.frame, forward)
                }
                Err(RecvTimeoutError::Timeout) => (Frame::Ping, None),
                Err(RecvTimeoutError::Disconnected) => {
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                    return;
                }
            };
        if matches!(frame, Frame::Ping) {
            next_ping = Instant::now() + PING_INTERVAL;
        }
        let started = stats.as_ref().map(|_| Instant::now());
        let result = if let Some(forward) = trace {
            let write = Span::enter_with_parent("server.write", &forward);
            match SpanContext::from_span(&write) {
                Some(context) => proto::write_trace_frame(&mut stream, &frame, context),
                None => proto::write_frame(&mut stream, &frame),
            }
        } else {
            proto::write_frame(&mut stream, &frame)
        };
        if let (Some(stats), Some(started)) = (stats.as_mut(), started) {
            stats.record(1, started.elapsed());
            stats.report_if_due();
        }
        if result.is_err() {
            let _ = net_tx.send(NetMsg::Dead(id));
            return;
        }
    }
}

fn reader_loop(mut stream: TcpStream, tx: Sender<QueuedFrame>, net_tx: Sender<NetMsg>, id: u64) {
    // The first frame must be a version-matching Hello, otherwise the
    // connection is rejected at once. A connection that fails the
    // handshake must not displace the serving one. This is admission
    // control at the protocol level: it keeps other clients on the same
    // port (such as deskflow's retry loop) from knocking out a healthy
    // connection.
    let rejection = match proto::read_frame(&mut stream) {
        Ok(Frame::Hello { version, openlogi })
            if matches!(version, proto::VERSION | proto::TRACE_VERSION) =>
        {
            let _ = net_tx.send(NetMsg::Hello {
                id,
                openlogi,
                supports_trace: version == proto::TRACE_VERSION,
            });
            None
        }
        Ok(Frame::Hello { version, .. }) => Some(Rejection::Version(version)),
        Ok(_) => Some(Rejection::Unexpected),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Some(Rejection::Closed),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ) =>
        {
            Some(Rejection::Silent)
        }
        Err(_) => Some(Rejection::Unexpected),
    };
    if let Some(reason) = rejection {
        let _ = net_tx.send(NetMsg::Rejected(id, reason));
        return;
    }
    loop {
        match proto::read_frame(&mut stream) {
            Ok(Frame::Ping) => {
                let _ = tx.send(QueuedFrame {
                    frame: Frame::Pong,
                    queued_at: None,
                    trace: None,
                });
            }
            Ok(Frame::ReturnToLocal) => {
                let _ = net_tx.send(NetMsg::ReturnToLocal(id));
            }
            // Hello, Pong and other frames do not concern the main loop;
            // ignore them.
            Ok(_) => {}
            Err(_) => {
                let _ = net_tx.send(NetMsg::Dead(id));
                return;
            }
        }
    }
}
