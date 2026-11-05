//! Exercise the clipboard service over real TCP, with an observable local clipboard.
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use hidpipe_proto::clipboard::{self, Clipboard, MAX_TEXT_BYTES};

struct MemoryClipboard {
    text: Arc<Mutex<Option<String>>>,
    writes: mpsc::Sender<String>,
    reads: mpsc::Sender<()>,
}

impl Clipboard for MemoryClipboard {
    fn read(&mut self) -> io::Result<Option<String>> {
        let text = self.text.lock().unwrap().clone();
        let _ = self.reads.send(());
        Ok(text)
    }

    fn write(&mut self, text: &str) -> io::Result<()> {
        *self.text.lock().unwrap() = Some(text.to_owned());
        self.writes.send(text.to_owned()).unwrap();
        Ok(())
    }
}

struct Harness {
    peer: TcpStream,
    text: Arc<Mutex<Option<String>>>,
    writes: mpsc::Receiver<String>,
    reads: mpsc::Receiver<()>,
    worker: Option<thread::JoinHandle<io::Result<()>>>,
}

impl Harness {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        peer.set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let (server, _) = listener.accept().unwrap();
        let text = Arc::new(Mutex::new(Some("existing local text".to_owned())));
        let (write_tx, writes) = mpsc::channel();
        let (read_tx, reads) = mpsc::channel();
        let mut clipboard = MemoryClipboard {
            text: Arc::clone(&text),
            writes: write_tx,
            reads: read_tx,
        };
        let worker = thread::spawn(move || clipboard::session(server, &mut clipboard));
        let mut magic = [0; 8];
        peer.read_exact(&mut magic).unwrap();
        assert_eq!(&magic, b"HIDCLIP\x01");
        peer.write_all(&magic).unwrap();
        reads.recv_timeout(Duration::from_secs(3)).unwrap();
        Self {
            peer,
            text,
            writes,
            reads,
            worker: Some(worker),
        }
    }

    fn send(&mut self, text: &str) {
        self.peer
            .write_all(&(text.len() as u32).to_be_bytes())
            .unwrap();
        self.peer.write_all(text.as_bytes()).unwrap();
    }

    fn receive(&mut self) -> String {
        loop {
            let mut size = [0; 4];
            self.peer.read_exact(&mut size).unwrap();
            let size = u32::from_be_bytes(size);
            if size == u32::MAX {
                continue;
            }
            assert!(size as usize <= MAX_TEXT_BYTES);
            let mut text = vec![0; size as usize];
            self.peer.read_exact(&mut text).unwrap();
            return String::from_utf8(text).unwrap();
        }
    }

    fn no_record(&mut self) {
        self.peer
            .set_read_timeout(Some(Duration::from_millis(400)))
            .unwrap();
        loop {
            let mut header = [0; 4];
            match self.peer.read_exact(&mut header) {
                Ok(()) => assert_eq!(
                    u32::from_be_bytes(header),
                    u32::MAX,
                    "unexpected text record"
                ),
                Err(error) => {
                    assert!(matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ));
                    break;
                }
            }
        }
        self.peer
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
    }

    fn finish(&mut self) -> io::Result<()> {
        self.peer.shutdown(Shutdown::Both).unwrap();
        self.worker.take().unwrap().join().unwrap()
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.peer.shutdown(Shutdown::Both);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[test]
fn shares_changes_both_ways_without_replaying_or_echoing() {
    let mut harness = Harness::new();
    harness.no_record();
    *harness.text.lock().unwrap() = Some("本地复制\nwith trailing newline\n".to_owned());
    assert_eq!(harness.receive(), "本地复制\nwith trailing newline\n");
    harness.send("远端复制 🦀\n");
    assert_eq!(
        harness.writes.recv_timeout(Duration::from_secs(3)).unwrap(),
        "远端复制 🦀\n"
    );
    harness.no_record();
    harness.send("");
    assert_eq!(
        harness.writes.recv_timeout(Duration::from_secs(3)).unwrap(),
        ""
    );
    harness.no_record();
    // Repeated remote contents must not reacquire clipboard ownership.
    harness.send("");
    assert!(harness
        .writes
        .recv_timeout(Duration::from_millis(300))
        .is_err());
    harness.finish().unwrap();
}

#[test]
fn preserves_partial_records_across_read_timeouts() {
    let mut harness = Harness::new();
    harness.peer.write_all(&[0, 0]).unwrap();
    // Wait for local polling, which requires crossing several read timeouts.
    harness.reads.recv_timeout(Duration::from_secs(3)).unwrap();
    harness.peer.write_all(&[0, 6, 0xe4]).unwrap();
    harness.reads.recv_timeout(Duration::from_secs(3)).unwrap();
    harness
        .peer
        .write_all(&[0xbd, 0xa0, 0xe5, 0xa5, 0xbd])
        .unwrap();
    assert_eq!(
        harness.writes.recv_timeout(Duration::from_secs(3)).unwrap(),
        "你好"
    );
    harness.finish().unwrap();
}

#[test]
fn exchanges_maximum_size_text_while_receiving() {
    let mut harness = Harness::new();
    *harness.text.lock().unwrap() = Some("L".repeat(MAX_TEXT_BYTES));
    // Wait until the local text has been sampled and queued for transmission.
    harness.reads.recv_timeout(Duration::from_secs(3)).unwrap();
    let mut sender = harness.peer.try_clone().unwrap();
    let sending = thread::spawn(move || {
        sender
            .write_all(&(MAX_TEXT_BYTES as u32).to_be_bytes())
            .unwrap();
        sender.write_all(&vec![b'R'; MAX_TEXT_BYTES]).unwrap();
    });
    assert_eq!(harness.receive(), "L".repeat(MAX_TEXT_BYTES));
    assert_eq!(
        harness.writes.recv_timeout(Duration::from_secs(3)).unwrap(),
        "R".repeat(MAX_TEXT_BYTES)
    );
    sending.join().unwrap();
    harness.finish().unwrap();
}

#[test]
fn rejects_oversized_and_non_utf8_records_without_writing() {
    for record in [
        ((MAX_TEXT_BYTES + 1) as u32).to_be_bytes().to_vec(),
        vec![0, 0, 0, 1, 0xff],
    ] {
        let mut harness = Harness::new();
        harness.peer.write_all(&record).unwrap();
        let error = harness.worker.take().unwrap().join().unwrap().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(harness.writes.try_recv().is_err());
    }
}

#[test]
fn rejects_other_protocols_before_accessing_clipboard() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (server, _) = listener.accept().unwrap();
    let (writes, _) = mpsc::channel();
    let (reads, read_rx) = mpsc::channel();
    let mut clipboard = MemoryClipboard {
        text: Arc::new(Mutex::new(None)),
        writes,
        reads,
    };
    let worker = thread::spawn(move || clipboard::session(server, &mut clipboard));
    peer.write_all(b"WRONG\x00\x00\x00").unwrap();
    assert_eq!(
        worker.join().unwrap().unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    assert!(read_rx.try_recv().is_err());
}
