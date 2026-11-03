//! Ordered input processing, independent of the platform injection API.

use std::io;
use std::net::TcpStream;
use std::time::Instant;

use hidpipe_proto as proto;
use proto::{bridge::Resolver, Frame, PointerEvent};

pub fn run(
    stream: &mut TcpStream,
    mut resolver: Option<&mut Resolver>,
    input_feedback: bool,
    latency_stats: bool,
    mut inject: impl FnMut(Frame) -> Option<Frame>,
    allowed: impl Fn() -> bool,
) -> io::Result<()> {
    let mut stats = latency_stats.then(|| {
        proto::latency::LatencyStats::new([
            "client.resolve",
            "client.inject",
            "client.feedback",
            "client.process",
        ])
    });
    loop {
        let frame = proto::read_frame(stream)?;
        let processing = stats.as_ref().map(|_| Instant::now());
        // Reply before resolution/injection so local processing cannot postpone
        // the packet intended to carry the TCP ACK. Ping keeps its own reply;
        // replying to Pong would create a feedback loop.
        if input_feedback
            && matches!(
                frame,
                Frame::Key { .. }
                    | Frame::Motion { .. }
                    | Frame::Button { .. }
                    | Frame::Wheel { .. }
                    | Frame::Pointer(_)
                    | Frame::ReleaseAll
            )
        {
            timed(&mut stats, 2, || proto::write_frame(stream, &Frame::Pong))?;
        }
        match frame {
            Frame::Ping => {
                if !allowed() {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "macOS input injection access was revoked",
                    ));
                }
                proto::write_frame(stream, &Frame::Pong)?;
            }
            Frame::Pointer(event) => {
                let resolver = resolver
                    .as_mut()
                    .ok_or_else(|| io::Error::other("server requires an OpenLogi bridge"))?;
                if timed(&mut stats, 0, || resolver.resolve(event))? {
                    if let Some(back) = timed(&mut stats, 1, || inject(frame)) {
                        proto::write_frame(stream, &back)?;
                    }
                }
            }
            Frame::ReleaseAll => {
                timed(&mut stats, 1, || inject(frame));
                if let Some(resolver) = resolver.as_mut() {
                    timed(&mut stats, 0, || resolver.resolve(PointerEvent::Reset))?;
                }
            }
            frame @ (Frame::Key { .. }
            | Frame::Motion { .. }
            | Frame::Button { .. }
            | Frame::Wheel { .. }) => {
                if let Some(back) = timed(&mut stats, 1, || inject(frame)) {
                    proto::write_frame(stream, &back)?;
                }
            }
            _ => {}
        }
        if let (Some(stats), Some(started)) = (stats.as_mut(), processing) {
            stats.record(3, started.elapsed());
            stats.report_if_due();
        }
    }
}

/// Keep the disabled diagnostics path free of clock reads and allocations.
fn timed<T>(
    stats: &mut Option<proto::latency::LatencyStats<4>>,
    stage: usize,
    action: impl FnOnce() -> T,
) -> T {
    let started = stats.as_ref().map(|_| Instant::now());
    let result = action();
    if let (Some(stats), Some(started)) = (stats.as_mut(), started) {
        stats.record(stage, started.elapsed());
    }
    result
}
