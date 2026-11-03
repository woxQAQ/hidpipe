//! Run the client's actual transport loop against TCP and Unix-socket peers.
#![cfg(unix)]

#[path = "../../client/src/transport.rs"]
mod transport;

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::net::UnixListener;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use hidpipe_proto::{self as proto, bridge::Resolver, Frame, KeyState, PointerEvent};

fn tcp_pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (server, _) = listener.accept().unwrap();
    for socket in [&client, &server] {
        socket.set_nodelay(true).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
    }
    (client, server)
}

#[test]
fn feedback_precedes_injection_and_can_be_disabled() {
    for feedback in [false, true] {
        let (mut client, mut server) = tcp_pair();
        let (entered, injection) = mpsc::channel();
        let (resume, resumed) = mpsc::channel();
        let frame = Frame::Key {
            usage: 4,
            state: KeyState::Press,
        };
        let worker = thread::spawn(move || {
            transport::run(
                &mut client,
                None,
                feedback,
                true,
                |received| {
                    entered.send(received).unwrap();
                    resumed.recv_timeout(Duration::from_secs(2)).unwrap();
                    Some(Frame::ReturnToLocal)
                },
                || true,
            )
        });
        proto::write_frame(&mut server, &frame).unwrap();
        assert_eq!(
            injection.recv_timeout(Duration::from_secs(2)).unwrap(),
            frame
        );
        if feedback {
            // Injection is deliberately blocked. Feedback must already be sent.
            assert_eq!(proto::read_frame(&mut server).unwrap(), Frame::Pong);
        }
        resume.send(()).unwrap();
        // With feedback disabled the first response is the handler's reply.
        assert_eq!(
            proto::read_frame(&mut server).unwrap(),
            Frame::ReturnToLocal
        );
        proto::write_frame(&mut server, &Frame::Pong).unwrap();
        proto::write_frame(&mut server, &Frame::Ping).unwrap();
        assert_eq!(proto::read_frame(&mut server).unwrap(), Frame::Pong);
        server.shutdown(Shutdown::Both).unwrap();
        assert!(worker.join().unwrap().is_err());
    }
}

#[test]
fn feedback_does_not_bypass_binding_disposition_or_reset() {
    let directory =
        std::env::temp_dir().join(format!("hidpipe-client-session-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let socket = directory.join("pointer.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let event = PointerEvent::Motion { dx: 20, dy: -10 };
    let (requested, requests) = mpsc::channel();
    let (resume, resumed) = mpsc::channel();
    let agent = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut hello = [0; 6];
        socket.read_exact(&mut hello).unwrap();
        assert_eq!(&hello[..5], &proto::pointer::MAGIC);
        assert_eq!(hello[5], 1);
        socket.write_all(&[0]).unwrap();
        assert_eq!(PointerEvent::read(&mut socket).unwrap(), event);
        requested.send(()).unwrap();
        resumed.recv_timeout(Duration::from_secs(2)).unwrap();
        socket.write_all(&[1]).unwrap(); // The binding consumes this motion.
        assert_eq!(
            PointerEvent::read(&mut socket).unwrap(),
            PointerEvent::Reset
        );
        socket.write_all(&[0]).unwrap();
    });
    let mut resolver = Resolver::connect(&socket).unwrap();
    let (mut client, mut server) = tcp_pair();
    let (injected, injections) = mpsc::channel();
    let worker = thread::spawn(move || {
        transport::run(
            &mut client,
            Some(&mut resolver),
            true,
            true,
            |frame| {
                injected.send(frame).unwrap();
                None
            },
            || true,
        )
    });
    proto::write_frame(&mut server, &Frame::Pointer(event)).unwrap();
    requests.recv_timeout(Duration::from_secs(2)).unwrap();
    // Feedback must not wait for the blocked agent's disposition.
    assert_eq!(proto::read_frame(&mut server).unwrap(), Frame::Pong);
    resume.send(()).unwrap();
    proto::write_frame(&mut server, &Frame::ReleaseAll).unwrap();
    assert_eq!(proto::read_frame(&mut server).unwrap(), Frame::Pong);
    assert_eq!(
        injections.recv_timeout(Duration::from_secs(2)).unwrap(),
        Frame::ReleaseAll
    );
    agent.join().unwrap();
    proto::write_frame(&mut server, &Frame::Ping).unwrap();
    assert_eq!(proto::read_frame(&mut server).unwrap(), Frame::Pong);
    server.shutdown(Shutdown::Both).unwrap();
    assert!(worker.join().unwrap().is_err());
    assert!(injections.try_recv().is_err());
    std::fs::remove_dir_all(directory).unwrap();
}
