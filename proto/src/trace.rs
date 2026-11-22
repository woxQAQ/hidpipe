//! OTLP/HTTP export runs on fastrace's reporting thread.

use std::{borrow::Cow, io, time::Duration};

use fastrace::collector::Config;
use fastrace_opentelemetry::OpenTelemetryReporter;
use opentelemetry::InstrumentationScope;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::Resource;

pub fn init(service: &'static str, endpoint: &str) -> io::Result<()> {
    if !endpoint.starts_with("http://") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "trace endpoint must be an http:// OTLP trace URL",
        ));
    }
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
        .with_timeout(Duration::from_secs(3))
        .build()
        .map_err(io::Error::other)?;
    let reporter = OpenTelemetryReporter::new(
        exporter,
        Cow::Owned(Resource::builder().with_service_name(service).build()),
        InstrumentationScope::builder("hidpipe").build(),
    );
    fastrace::set_reporter(reporter, Config::default());
    Ok(())
}
