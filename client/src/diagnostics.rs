//! Per-session injection timing. Quartz stages are measured on the injection thread.

use std::cell::Cell;
use std::fmt::Write;
use std::io;
use std::time::{Duration, Instant, SystemTime};

use fastrace::{collector::SpanContext, Span};
use hidpipe_proto::{Frame, PointerEvent};
use opentelemetry::logs::{LogRecord, Logger, LoggerProvider, Severity};
use opentelemetry::TraceFlags;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::logs::{SdkLogger, SdkLoggerProvider};
use opentelemetry_sdk::Resource;

const SLOW: Duration = Duration::from_millis(2);
const FIELDS: [&str; 6] = [
    "bounds_us",
    "state_us",
    "create_us",
    "configure_us",
    "post_us",
    "drop_us",
];

#[derive(Clone, Copy)]
pub enum Stage {
    Bounds,
    State,
    Create,
    Configure,
    Post,
    Drop,
}

thread_local! {
    static ACTIVE: Cell<Option<[Duration; 6]>> = const { Cell::new(None) };
    static PROCESS_SEQUENCE: Cell<u64> = const { Cell::new(0) };
}

pub fn log_provider(endpoint: &str) -> io::Result<SdkLoggerProvider> {
    if !endpoint.starts_with("http://") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected an http:// OTLP logs URL",
        ));
    }
    let exporter = opentelemetry_otlp::LogExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
        .with_timeout(Duration::from_secs(3))
        .build()
        .map_err(io::Error::other)?;
    Ok(SdkLoggerProvider::builder()
        .with_resource(
            Resource::builder()
                .with_service_name("hidpipe-client")
                .build(),
        )
        .with_batch_exporter(exporter)
        .build())
}

pub struct Session {
    started: Instant,
    input_seq: u64,
    latency_stats: bool,
    logger: Option<SdkLogger>,
}

impl Session {
    pub fn new(latency_stats: bool, provider: Option<&SdkLoggerProvider>) -> Self {
        Self {
            started: Instant::now(),
            input_seq: 0,
            latency_stats,
            logger: provider.map(|p| p.logger("hidpipe")),
        }
    }

    pub fn inject<T>(&mut self, frame: Frame, span: Option<Span>, action: impl FnOnce() -> T) -> T {
        self.input_seq += 1;
        if !self.latency_stats && self.logger.is_none() && span.is_none() {
            return action();
        }
        let started = Instant::now();
        ACTIVE.set(Some([Duration::ZERO; 6]));
        let result = action();
        let elapsed = started.elapsed();
        let stages = ACTIVE.take().expect("injection stage timing active");
        let kind = match frame {
            Frame::Motion { .. } | Frame::Pointer(PointerEvent::Motion { .. }) => "motion",
            Frame::Key { .. } => "key",
            Frame::Button { .. } | Frame::Pointer(PointerEvent::Button { .. }) => "button",
            Frame::Wheel { .. } | Frame::Pointer(PointerEvent::Wheel { .. }) => "wheel",
            _ => "control",
        };
        let age_ms = started.duration_since(self.started).as_millis() as i64;
        if let Some(span) = &span {
            span.add_properties(|| {
                [
                    ("input.kind", kind.to_owned()),
                    ("input_seq", self.input_seq.to_string()),
                    ("session_age_ms", age_ms.to_string()),
                    (
                        "create_us",
                        stages[Stage::Create as usize].as_micros().to_string(),
                    ),
                    (
                        "post_us",
                        stages[Stage::Post as usize].as_micros().to_string(),
                    ),
                ]
            });
        }
        // Preserve the process-wide sequence in the existing stderr format.
        let seq = PROCESS_SEQUENCE.with(|value| {
            let next = value.get() + 1;
            value.set(next);
            next
        });
        if elapsed >= SLOW && (self.latency_stats || self.logger.is_some()) {
            let context = span.as_ref().and_then(SpanContext::from_span);
            drop(span);
            self.report_slow(kind, seq, age_ms, elapsed, stages, context);
        }
        result
    }

    fn report_slow(
        &self,
        kind: &'static str,
        seq: u64,
        age_ms: i64,
        elapsed: Duration,
        stages: [Duration; 6],
        context: Option<SpanContext>,
    ) {
        let completed = SystemTime::now();
        let total_us = elapsed.as_micros() as i64;
        let other_us = elapsed.saturating_sub(stages.iter().sum()).as_micros() as i64;
        let fields = FIELDS.into_iter().zip(stages.map(|d| d.as_micros() as i64));
        if let Some(logger) = &self.logger {
            let mut record = logger.create_log_record();
            record.set_timestamp(completed);
            record.set_event_name("slow-inject");
            record.set_body("slow-inject".into());
            record.set_severity_number(Severity::Warn);
            record.add_attribute("input.kind", kind);
            record.add_attribute("input_seq", self.input_seq as i64);
            record.add_attribute("session_age_ms", age_ms);
            record.add_attribute("total_us", total_us);
            record.add_attributes(fields.clone());
            record.add_attribute("other_us", other_us);
            if let Some(context) = context {
                record.set_trace_context(
                    context.trace_id.0.into(),
                    context.span_id.0.into(),
                    Some(TraceFlags::SAMPLED),
                );
            }
            logger.emit(record);
        }
        let epoch_us = completed
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros();
        let mut details = String::new();
        for (name, value) in fields {
            write!(details, " {name}={value}").unwrap();
        }
        if let Some(context) = context {
            write!(
                details,
                " trace_id={} span_id={}",
                context.trace_id, context.span_id
            )
            .unwrap();
        }
        eprintln!("[{}] [slow-inject] epoch_us={epoch_us} seq={seq} kind={kind} total_us={total_us}{details} other_us={other_us} input_seq={} session_age_ms={age_ms}", crate::timestamp(), self.input_seq);
    }
}

/// Stages must not overlap; other_us includes work outside these scopes.
pub fn measure<T>(stage: Stage, action: impl FnOnce() -> T) -> T {
    let started = ACTIVE.with(|active| active.get().map(|_| Instant::now()));
    let result = action();
    if let Some(started) = started {
        let elapsed = started.elapsed();
        ACTIVE.with(|active| {
            if let Some(mut stages) = active.get() {
                stages[stage as usize] += elapsed;
                active.set(Some(stages));
            }
        });
    }
    result
}
