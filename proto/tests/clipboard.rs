//! Exercise typed clipboard sessions over real TCP with an observable clipboard.
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use hidpipe_proto::clipboard::{self, Clipboard, Content, MAX_IMAGE_BYTES, MAX_TEXT_BYTES};

struct MemoryClipboard {
    content: Arc<Mutex<Option<Content>>>,
    writes: mpsc::Sender<Content>,
    reads: mpsc::Sender<()>,
}

impl Clipboard for MemoryClipboard {
    fn read(&mut self) -> io::Result<Option<Content>> {
        let content = self.content.lock().unwrap().clone();
        let _ = self.reads.send(());
        Ok(content)
    }

    fn write(&mut self, content: &Content) -> io::Result<()> {
        *self.content.lock().unwrap() = Some(content.clone());
        self.writes.send(content.clone()).unwrap();
        Ok(())
    }
}

struct Harness {
    peer: TcpStream,
    content: Arc<Mutex<Option<Content>>>,
    writes: mpsc::Receiver<Content>,
    reads: mpsc::Receiver<()>,
    worker: Option<thread::JoinHandle<io::Result<()>>>,
}

fn record(content: &Content) -> Vec<u8> {
    let (kind, bytes) = match content {
        Content::Text(text) => (0, text.as_bytes()),
        Content::Png(bytes) => (1, bytes.as_slice()),
    };
    let mut record = ((bytes.len() + 1) as u32).to_be_bytes().to_vec();
    record.push(kind);
    record.extend_from_slice(bytes);
    record
}

impl Harness {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        peer.set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let (server, _) = listener.accept().unwrap();
        let content = Arc::new(Mutex::new(Some(Content::Text(
            "existing local text".into(),
        ))));
        let (write_tx, writes) = mpsc::channel();
        let (read_tx, reads) = mpsc::channel();
        let mut clipboard = MemoryClipboard {
            content: Arc::clone(&content),
            writes: write_tx,
            reads: read_tx,
        };
        let worker = thread::spawn(move || clipboard::session(server, &mut clipboard));
        let mut magic = [0; 8];
        peer.read_exact(&mut magic).unwrap();
        assert_eq!(&magic, b"HIDCLIP\x02");
        peer.write_all(&magic).unwrap();
        reads.recv_timeout(Duration::from_secs(3)).unwrap();
        Self {
            peer,
            content,
            writes,
            reads,
            worker: Some(worker),
        }
    }

    fn send(&mut self, content: &Content) {
        self.peer.write_all(&record(content)).unwrap();
    }

    fn receive(&mut self) -> Content {
        loop {
            let mut size = [0; 4];
            self.peer.read_exact(&mut size).unwrap();
            let size = u32::from_be_bytes(size);
            if size == u32::MAX {
                continue;
            }
            assert!(size as usize <= MAX_IMAGE_BYTES + 1);
            let mut payload = vec![0; size as usize];
            self.peer.read_exact(&mut payload).unwrap();
            return match payload.remove(0) {
                0 => Content::Text(String::from_utf8(payload).unwrap()),
                1 => Content::Png(payload),
                _ => panic!("unexpected content type"),
            };
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
                    "unexpected clipboard record"
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
            .set_read_timeout(Some(Duration::from_secs(5)))
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

// A complete 1x1 PNG, with binary bytes that cannot pass through a text channel.
fn png() -> Vec<u8> {
    vec![
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 4,
        0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 252, 255, 31, 0, 3, 3,
        2, 0, 239, 162, 167, 91, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ]
}

#[test]
fn shares_text_and_images_both_ways_without_replaying_or_echoing() {
    let mut harness = Harness::new();
    harness.no_record();
    for content in [
        Content::Text("本地复制 🦀\nwith trailing newline\n".into()),
        Content::Png(png()),
        Content::Text(String::new()),
    ] {
        *harness.content.lock().unwrap() = Some(content.clone());
        assert_eq!(harness.receive(), content);
        let remote = match &content {
            Content::Text(text) => Content::Text(format!("remote {text}")),
            Content::Png(bytes) => {
                let mut bytes = bytes.clone();
                bytes.extend_from_slice(b"remote");
                Content::Png(bytes)
            }
        };
        harness.send(&remote);
        assert_eq!(
            harness.writes.recv_timeout(Duration::from_secs(3)).unwrap(),
            remote
        );
        harness.no_record();
        harness.send(&remote);
        assert!(harness
            .writes
            .recv_timeout(Duration::from_millis(300))
            .is_err());
    }
    harness.send(&Content::Text(String::new()));
    assert_eq!(
        harness.writes.recv_timeout(Duration::from_secs(3)).unwrap(),
        Content::Text(String::new())
    );
    harness.finish().unwrap();
}

#[test]
fn preserves_partial_image_records_across_read_timeouts() {
    let mut harness = Harness::new();
    let image = Content::Png(png());
    let record = record(&image);
    harness.peer.write_all(&record[..2]).unwrap();
    harness.reads.recv_timeout(Duration::from_secs(3)).unwrap();
    harness.peer.write_all(&record[2..7]).unwrap();
    harness.reads.recv_timeout(Duration::from_secs(3)).unwrap();
    harness.peer.write_all(&record[7..]).unwrap();
    assert_eq!(
        harness.writes.recv_timeout(Duration::from_secs(3)).unwrap(),
        image
    );
    harness.finish().unwrap();
}

#[test]
fn exchanges_maximum_size_payloads_while_receiving() {
    for local in [
        Content::Text("L".repeat(MAX_TEXT_BYTES)),
        Content::Png({
            let mut bytes = png();
            bytes.resize(MAX_IMAGE_BYTES, 0);
            bytes
        }),
    ] {
        let mut harness = Harness::new();
        *harness.content.lock().unwrap() = Some(local.clone());
        harness.reads.recv_timeout(Duration::from_secs(3)).unwrap();
        let remote = match &local {
            Content::Text(_) => Content::Text("R".repeat(MAX_TEXT_BYTES)),
            Content::Png(_) => {
                let mut bytes = png();
                bytes.resize(MAX_IMAGE_BYTES, 1);
                Content::Png(bytes)
            }
        };
        let outgoing = record(&remote);
        let mut sender = harness.peer.try_clone().unwrap();
        let sending = thread::spawn(move || sender.write_all(&outgoing).unwrap());
        assert_eq!(harness.receive(), local);
        assert_eq!(
            harness.writes.recv_timeout(Duration::from_secs(5)).unwrap(),
            remote
        );
        sending.join().unwrap();
        harness.finish().unwrap();
    }
}

#[test]
fn rejects_invalid_records_without_writing() {
    let mut oversized_text = ((MAX_TEXT_BYTES + 2) as u32).to_be_bytes().to_vec();
    oversized_text.push(0);
    for record in [
        ((MAX_IMAGE_BYTES + 2) as u32).to_be_bytes().to_vec(),
        oversized_text,
        vec![0, 0, 0, 0],
        vec![0, 0, 0, 2, 0, 0xff],
        vec![0, 0, 0, 1, 2],
        vec![0, 0, 0, 2, 1, 0xff],
    ] {
        let mut harness = Harness::new();
        harness.peer.write_all(&record).unwrap();
        let error = harness.worker.take().unwrap().join().unwrap().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(harness.writes.try_recv().is_err());
    }
}

#[test]
fn rejects_old_protocol_before_accessing_clipboard() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (server, _) = listener.accept().unwrap();
    let (writes, _) = mpsc::channel();
    let (reads, read_rx) = mpsc::channel();
    let mut clipboard = MemoryClipboard {
        content: Arc::new(Mutex::new(None)),
        writes,
        reads,
    };
    let worker = thread::spawn(move || clipboard::session(server, &mut clipboard));
    peer.write_all(b"HIDCLIP\x01").unwrap();
    assert_eq!(
        worker.join().unwrap().unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    assert!(read_rx.try_recv().is_err());
}
