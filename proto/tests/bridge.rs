//! Exercise stream framing and failure behavior at the actual Unix socket boundary.
#![cfg(unix)]

use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use hidpipe_proto::bridge::{Capture, Resolver};
use hidpipe_proto::pointer::{PointerEvent, MAGIC};
use hidpipe_proto::{read_frame, write_frame, Frame};

#[test]
fn fragmented_capture_records_survive_network_forwarding_and_eof() {
    let directory =
        std::env::temp_dir().join(format!("hidpipe-capture-test-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let socket = directory.join("pointer.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let event = PointerEvent::GestureMotion {
        dx: i32::MIN,
        dy: i32::MAX,
    };
    let (written, ready) = mpsc::channel();
    let (resume, resumed) = mpsc::channel();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut hello = [0; 6];
        stream.read_exact(&mut hello).unwrap();
        assert_eq!(&hello[..5], &MAGIC);
        assert_eq!(hello[5], 0);
        stream.write_all(&[0]).unwrap();
        let bytes = event.encode();
        stream.write_all(&bytes[..3]).unwrap();
        written.send(()).unwrap();
        resumed.recv_timeout(Duration::from_secs(2)).unwrap();
        stream.write_all(&bytes[3..]).unwrap();
    });
    let mut capture = Capture::connect(&socket).unwrap();
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(
        capture.poll().unwrap(),
        None,
        "partial input is retained, never decoded early"
    );
    resume.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let captured = loop {
        if let Some(event) = capture.poll().unwrap() {
            break event;
        }
        assert!(Instant::now() < deadline);
        thread::yield_now();
    };
    let mut network = Vec::new();
    write_frame(&mut network, &Frame::Pointer(captured)).unwrap();
    assert_eq!(
        read_frame(&mut network.as_slice()).unwrap(),
        Frame::Pointer(event)
    );
    worker.join().unwrap();
    assert_eq!(
        capture.poll().unwrap_err().kind(),
        std::io::ErrorKind::UnexpectedEof
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn resolver_rejects_malformed_responses_instead_of_injecting_input() {
    let directory =
        std::env::temp_dir().join(format!("hidpipe-resolver-test-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let socket = directory.join("pointer.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut hello = [0; 6];
        stream.read_exact(&mut hello).unwrap();
        assert_eq!(&hello[..5], &MAGIC);
        assert_eq!(hello[5], 1);
        stream.write_all(&[0]).unwrap();
        assert_eq!(
            PointerEvent::read(&mut stream).unwrap(),
            PointerEvent::Reset
        );
        stream.write_all(&[255]).unwrap();
    });
    let mut resolver = Resolver::connect(&socket).unwrap();
    assert_eq!(
        resolver.resolve(PointerEvent::Reset).unwrap_err().kind(),
        std::io::ErrorKind::InvalidData
    );
    worker.join().unwrap();
    std::fs::remove_dir_all(directory).unwrap();
}
