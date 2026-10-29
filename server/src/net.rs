//! Read/write threads for a single client TCP connection.
//!
//! Keepalive: the writer sends a Ping every 2s; the reader declares the
//! connection dead after 6s without any frame and reports
//! `NetMsg::Dead(connection generation)`. The main loop uses the generation
//! to ignore late death notices from superseded connections.

use std::io;
use std::net::TcpStream;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use hidpipe_proto as proto;
use proto::Frame;

const PING_INTERVAL: Duration = Duration::from_secs(2);
const READ_TIMEOUT: Duration = Duration::from_secs(6);

pub enum NetMsg {
    /// The pending connection's first frame was a version-matching Hello;
    /// it may take over.
    Hello(u64),
    /// The client asks to hand control back (cursor pushed against the
    /// Mac's left edge).
    ReturnToLocal(u64),
    /// The connection is dead (read timeout, write failure, peer closed,
    /// bad handshake). Carries the connection generation.
    Dead(u64),
}

pub struct Connection {
    pub id: u64,
    tx: Sender<Frame>,
}

impl Connection {
    pub fn spawn(stream: TcpStream, net_tx: Sender<NetMsg>, id: u64) -> io::Result<Self> {
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(READ_TIMEOUT))?;
        let reader = stream.try_clone()?;
        let (tx, rx) = mpsc::channel::<Frame>();
        thread::spawn({
            let net_tx = net_tx.clone();
            move || writer_loop(stream, rx, net_tx, id)
        });
        thread::spawn({
            let tx = tx.clone();
            move || reader_loop(reader, tx, net_tx, id)
        });
        Ok(Self { id, tx })
    }

    /// A failed send means the peer is dead; the read/write threads report
    /// Dead shortly after, so the frame is dropped silently here.
    pub fn send(&self, frame: Frame) {
        let _ = self.tx.send(frame);
    }
}

// When the main loop drops the Connection the channel closes and the
// writer exits; the reader exits at the latest after one read timeout (6s).

fn writer_loop(mut stream: TcpStream, rx: Receiver<Frame>, net_tx: Sender<NetMsg>, id: u64) {
    // Pings must go out on schedule, independent of data traffic. If we
    // only pinged after 2s of channel idle, constant mouse motion would
    // starve Ping forever: the peer would never see a Ping, never answer
    // Pong, and our reader would wrongly declare the connection dead after
    // 6s.
    let mut next_ping = Instant::now() + PING_INTERVAL;
    loop {
        let frame = match rx.recv_timeout(next_ping.saturating_duration_since(Instant::now())) {
            Ok(frame) => frame,
            Err(RecvTimeoutError::Timeout) => Frame::Ping,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        if matches!(frame, Frame::Ping) {
            next_ping = Instant::now() + PING_INTERVAL;
        }
        if proto::write_frame(&mut stream, &frame).is_err() {
            let _ = net_tx.send(NetMsg::Dead(id));
            return;
        }
    }
}

fn reader_loop(mut stream: TcpStream, tx: Sender<Frame>, net_tx: Sender<NetMsg>, id: u64) {
    // The first frame must be a version-matching Hello, otherwise the
    // connection is declared dead at once. A connection that fails the
    // handshake must not displace the serving one. This is admission
    // control at the protocol level: it keeps other clients on the same
    // port (such as deskflow's retry loop) from knocking out a healthy
    // connection.
    match proto::read_frame(&mut stream) {
        Ok(Frame::Hello { version }) if version == proto::VERSION => {
            let _ = net_tx.send(NetMsg::Hello(id));
        }
        _ => {
            let _ = net_tx.send(NetMsg::Dead(id));
            return;
        }
    }
    loop {
        match proto::read_frame(&mut stream) {
            Ok(Frame::Ping) => {
                let _ = tx.send(Frame::Pong);
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
