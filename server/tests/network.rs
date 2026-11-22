//! Exercise connection admission, input ordering and feedback on real TCP sockets.

#[allow(dead_code)]
#[path = "../../client/src/diagnostics.rs"]
mod diagnostics;
#[path = "../src/net.rs"]
mod net;
#[path = "../../client/src/transport.rs"]
mod transport;

use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::Duration;

use hidpipe_proto::{self as proto, Frame, KeyState};
use net::{Connection, NetMsg, Rejection};

fn timestamp() -> String {
    "network-test".into()
}

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
    let mut connection = Connection::spawn(stream, tx, 42, true).unwrap();
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
        NetMsg::Hello {
            id: 42,
            openlogi: true,
            supports_trace: false
        }
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

/// Exercise both process-local reporters and the parent IDs received by an OTLP sink.
#[test]
fn sampled_inputs_cross_processes() {
    use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
    use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
    use opentelemetry_proto::tonic::common::v1::any_value::Value;
    use prost::Message;
    use std::{process::Command, sync::Arc, thread};

    const CHILD_ADDR: &str = "HIDPIPE_TEST_INPUT_ADDR";
    const ENDPOINT: &str = "HIDPIPE_TEST_OTLP_URL";
    let timeout = Duration::from_secs(5);
    let frames = [
        Frame::Motion { dx: 1, dy: 0 },
        Frame::ReleaseAll,
        Frame::Motion { dx: 2, dy: 0 },
    ];
    if let Ok(addr) = std::env::var(CHILD_ADDR) {
        let url = std::env::var(ENDPOINT).unwrap();
        proto::trace::init("hidpipe-client", &url).unwrap();
        let provider = diagnostics::log_provider(&url.replace("/v1/traces", "/v1/logs")).unwrap();
        let mut session = diagnostics::Session::new(false, Some(&provider));
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(timeout)).unwrap();
        proto::write_frame(
            &mut stream,
            &Frame::Hello {
                version: proto::TRACE_VERSION,
                openlogi: false,
            },
        )
        .unwrap();
        let mut injected = Vec::new();
        let result = transport::run(
            &mut stream,
            None,
            transport::Options {
                input_feedback: true,
                latency_stats: false,
                trace_enabled: true,
            },
            |frame, span| {
                session.inject(frame, span, || {
                    // Delay one sampled and one ordinary input inside the creation stage.
                    if injected.len() < 2 {
                        diagnostics::measure(diagnostics::Stage::Create, || {
                            thread::sleep(Duration::from_millis(4))
                        });
                    }
                    injected.push(frame);
                    (injected.len() == frames.len()).then_some(Frame::ReturnToLocal)
                })
            },
            || true,
        );
        assert_eq!(
            result.unwrap_err().kind(),
            std::io::ErrorKind::UnexpectedEof
        );
        assert_eq!(injected, frames);
        fastrace::flush();
        provider.shutdown().unwrap();
        return;
    }

    let collector = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
    let url = format!("http://{}/v1/traces", collector.server_addr());
    let sink = thread::spawn({
        let collector = Arc::clone(&collector);
        move || {
            let mut spans = Vec::new();
            let mut logs = Vec::new();
            while let Ok(mut request) = collector.recv() {
                let mut bytes = Vec::new();
                request.as_reader().read_to_end(&mut bytes).unwrap();
                if request.url() == "/v1/logs" {
                    for resource in ExportLogsServiceRequest::decode(&*bytes)
                        .unwrap()
                        .resource_logs
                    {
                        for scope in resource.scope_logs {
                            logs.extend(scope.log_records);
                        }
                    }
                    request.respond(tiny_http::Response::empty(200)).unwrap();
                    continue;
                }
                assert_eq!(request.url(), "/v1/traces");
                let export = ExportTraceServiceRequest::decode(&*bytes).unwrap();
                for resource in export.resource_spans {
                    let service = resource
                        .resource
                        .unwrap()
                        .attributes
                        .into_iter()
                        .find(|attr| attr.key == "service.name")
                        .unwrap()
                        .value
                        .unwrap()
                        .value
                        .unwrap();
                    let Value::StringValue(service) = service else {
                        panic!("invalid service name")
                    };
                    for scope in resource.scope_spans {
                        spans.extend(scope.spans.into_iter().map(|span| (service.clone(), span)));
                    }
                }
                request.respond(tiny_http::Response::empty(200)).unwrap();
            }
            (spans, logs)
        }
    });
    proto::trace::init("hidpipe-server", &url).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (accepted, socket) = mpsc::channel();
    thread::spawn(move || accepted.send(listener.accept()).ok());
    let mut client = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "sampled_inputs_cross_processes"])
        .env(CHILD_ADDR, addr.to_string())
        .env(ENDPOINT, url)
        .spawn()
        .unwrap();
    let (stream, _) = socket.recv_timeout(timeout).unwrap().unwrap();
    let shutdown = stream.try_clone().unwrap();
    let (tx, rx) = mpsc::channel();
    let mut conn = Connection::spawn(stream, tx, 1, false).unwrap();
    assert!(matches!(
        rx.recv_timeout(timeout).unwrap(),
        NetMsg::Hello {
            supports_trace: true,
            ..
        }
    ));
    conn.trace_every = std::num::NonZeroU32::new(2);
    for frame in frames {
        conn.send(frame);
    }
    assert!(matches!(
        rx.recv_timeout(timeout).unwrap(),
        NetMsg::ReturnToLocal(1)
    ));
    shutdown.shutdown(Shutdown::Write).unwrap();
    assert!(client.wait().unwrap().success());
    fastrace::flush();
    collector.unblock();
    let (records, logs) = sink.join().unwrap();
    let field = |attrs: &[opentelemetry_proto::tonic::common::v1::KeyValue], key: &str| {
        attrs
            .iter()
            .find(|a| a.key == key)
            .unwrap()
            .value
            .as_ref()
            .unwrap()
            .value
            .clone()
            .unwrap()
    };
    for (seq, kind) in [(1, "motion"), (2, "control")] {
        let log = logs
            .iter()
            .find(|log| field(&log.attributes, "input_seq") == Value::IntValue(seq))
            .unwrap();
        assert_eq!(log.event_name, "slow-inject");
        assert_eq!(
            field(&log.attributes, "input.kind"),
            Value::StringValue(kind.into())
        );
        assert!(matches!(field(&log.attributes, "create_us"), Value::IntValue(us) if us >= 2000));
        if seq == 1 {
            let (_, injection) = records
                .iter()
                .find(|(_, s)| s.name == "client.inject" && s.span_id == log.span_id)
                .unwrap();
            assert_eq!(log.trace_id, injection.trace_id);
            assert_eq!(
                field(&injection.attributes, "input.kind"),
                Value::StringValue("motion".into())
            );
            assert_eq!(
                field(&injection.attributes, "input_seq"),
                Value::StringValue("1".into())
            );
            assert!(
                matches!(field(&injection.attributes, "create_us"), Value::StringValue(v) if v.parse::<u64>().unwrap() >= 2000)
            );
            for key in ["post_us", "session_age_ms"] {
                assert!(matches!(
                    field(&injection.attributes, key),
                    Value::StringValue(_)
                ));
            }
        } else {
            assert!(log.trace_id.is_empty() && log.span_id.is_empty());
        }
    }
    let roots: Vec<_> = records
        .iter()
        .filter(|(_, s)| s.name == "server.forward")
        .collect();
    assert_eq!(roots.len(), 2);
    assert_ne!(roots[0].1.trace_id, roots[1].1.trace_id);
    for (_, root) in roots {
        let span = |service: &str, name: &str| {
            let mut matches = records
                .iter()
                .filter(|(svc, s)| svc == service && s.trace_id == root.trace_id && s.name == name);
            let (_, record) = matches.next().unwrap();
            assert!(matches.next().is_none());
            record
        };
        let write = span("hidpipe-server", "server.write");
        let process = span("hidpipe-client", "client.process");
        assert_eq!(
            span("hidpipe-server", "server.queue").parent_span_id,
            root.span_id
        );
        assert_eq!(write.parent_span_id, root.span_id);
        assert_eq!(process.parent_span_id, write.span_id);
        assert_eq!(
            span("hidpipe-client", "client.inject").parent_span_id,
            process.span_id
        );
        assert_eq!(
            span("hidpipe-client", "client.feedback").parent_span_id,
            process.span_id
        );
    }
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
