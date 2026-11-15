//! Exercise connection admission, input ordering and feedback on real TCP sockets.

#[path = "../src/net.rs"]
mod net;

use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::Duration;

use hidpipe_proto::{self as proto, Frame, KeyState};
use net::{Connection, NetMsg, Rejection};

#[test]
fn feedback_preserves_input_order_and_does_not_echo() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    client
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    client
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let (stream, _) = listener.accept().unwrap();
    let (tx, rx) = mpsc::channel();
    let connection = Connection::spawn(stream, tx, 42, true).unwrap();
    assert_eq!(connection.id, 42);
    proto::write_frame(
        &mut client,
        &Frame::Hello {
            version: proto::VERSION,
            openlogi: true,
        },
    )
    .unwrap();
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        NetMsg::Hello(42, true)
    ));

    // The Mac's per-input Pong must never create a reply loop or consume
    // input frames queued by the capture thread.
    let frames = [
        Frame::Motion { dx: 12, dy: -8 },
        Frame::Key {
            usage: 4,
            state: KeyState::Press,
        },
        Frame::Key {
            usage: 4,
            state: KeyState::Release,
        },
        Frame::ReleaseAll,
    ];
    for frame in frames {
        connection.send(frame);
    }
    for frame in frames {
        assert_eq!(proto::read_frame(&mut client).unwrap(), frame);
        proto::write_frame(&mut client, &Frame::Pong).unwrap();
    }
    let error = proto::read_frame(&mut client).unwrap_err();
    assert!(matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    ));

    proto::write_frame(&mut client, &Frame::Ping).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    assert_eq!(proto::read_frame(&mut client).unwrap(), Frame::Pong);
    proto::write_frame(&mut client, &Frame::ReturnToLocal).unwrap();
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        NetMsg::ReturnToLocal(42)
    ));
    client.shutdown(Shutdown::Both).unwrap();
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        NetMsg::Dead(42)
    ));
    drop(connection);
}

/// Admission control: a peer that cannot handshake is rejected with a reason
/// and never reaches the serving slot.
#[test]
fn rejection_reports_why_admission_failed() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();

    let closed = TcpStream::connect(address).unwrap();
    let (stream, _) = listener.accept().unwrap();
    let (tx, rx) = mpsc::channel();
    let connection = Connection::spawn(stream, tx, 1, false).unwrap();
    closed.shutdown(Shutdown::Both).unwrap();
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        NetMsg::Rejected(1, Rejection::Closed)
    ));
    drop(connection);

    let mut outdated = TcpStream::connect(address).unwrap();
    outdated
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let (stream, _) = listener.accept().unwrap();
    let (tx, rx) = mpsc::channel();
    let connection = Connection::spawn(stream, tx, 2, false).unwrap();
    proto::write_frame(
        &mut outdated,
        &Frame::Hello {
            version: proto::VERSION - 1,
            openlogi: false,
        },
    )
    .unwrap();
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        NetMsg::Rejected(2, Rejection::Version(version)) if version == proto::VERSION - 1
    ));
    drop(connection);

    let mut foreign = TcpStream::connect(address).unwrap();
    let (stream, _) = listener.accept().unwrap();
    let (tx, rx) = mpsc::channel();
    let connection = Connection::spawn(stream, tx, 3, false).unwrap();
    proto::write_frame(&mut foreign, &Frame::Ping).unwrap();
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        NetMsg::Rejected(3, Rejection::Unexpected)
    ));
    drop(connection);
}
