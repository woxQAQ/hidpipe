//! Ordered input processing, independent of the platform injection API.

use std::io;
use std::net::TcpStream;
use std::time::Instant;

use fastrace::Span;
use hidpipe_proto::{self as proto};
use proto::{bridge::Resolver, latency::LatencyStats, Frame, PointerEvent};

const STAGES: [&str; 4] = [
    "client.resolve",
    "client.inject",
    "client.feedback",
    "client.process",
];

pub struct Options {
    pub input_feedback: bool,
    pub latency_stats: bool,
    pub trace_enabled: bool,
}

pub fn run(
    stream: &mut TcpStream,
    mut resolver: Option<&mut Resolver>,
    options: Options,
    mut inject: impl FnMut(Frame, Option<Span>) -> Option<Frame>,
    allowed: impl Fn() -> bool,
) -> io::Result<()> {
    let mut stats = options.latency_stats.then(|| LatencyStats::new(STAGES));
    loop {
        let (frame, parent) = if options.trace_enabled {
            proto::read_trace_frame(stream)?
        } else {
            (proto::read_frame(stream)?, None)
        };
        let span = parent.map(|context| {
            Span::root(STAGES[3], context).with_property(|| ("span.parent_span_is_remote", "true"))
        });
        let mut timing = FrameTiming {
            stats: stats.as_mut(),
            span,
        };
        let processing = timing.stats.as_ref().map(|_| Instant::now());
        // Reply before resolution/injection so local processing cannot postpone
        // the packet intended to carry the TCP ACK. Ping keeps its own reply;
        // replying to Pong would create a feedback loop.
        if options.input_feedback && frame.is_input() {
            timing.stage(2, |_| proto::write_frame(stream, &Frame::Pong))?;
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
                if timing.stage(0, |_| resolver.resolve(event))? {
                    if let Some(back) = timing.stage(1, |span| inject(frame, span)) {
                        proto::write_frame(stream, &back)?;
                    }
                }
            }
            Frame::ReleaseAll => {
                timing.stage(1, |span| inject(frame, span));
                if let Some(resolver) = resolver.as_mut() {
                    timing.stage(0, |_| resolver.resolve(PointerEvent::Reset))?;
                }
            }
            frame @ (Frame::Key { .. }
            | Frame::Motion { .. }
            | Frame::Button { .. }
            | Frame::Wheel { .. }) => {
                if let Some(back) = timing.stage(1, |span| inject(frame, span)) {
                    proto::write_frame(stream, &back)?;
                }
            }
            _ => {}
        }
        if let (Some(stats), Some(started)) = (timing.stats, processing) {
            stats.record(3, started.elapsed());
            stats.report_if_due();
        }
    }
}

struct FrameTiming<'a> {
    stats: Option<&'a mut LatencyStats<4>>,
    span: Option<Span>,
}

impl FrameTiming<'_> {
    fn stage<T>(&mut self, stage: usize, action: impl FnOnce(Option<Span>) -> T) -> T {
        let span = self
            .span
            .as_ref()
            .map(|parent| Span::enter_with_parent(STAGES[stage], parent));
        let started = self.stats.as_ref().map(|_| Instant::now());
        let result = action(span);
        if let (Some(stats), Some(started)) = (self.stats.as_mut(), started) {
            stats.record(stage, started.elapsed());
        }
        result
    }
}
