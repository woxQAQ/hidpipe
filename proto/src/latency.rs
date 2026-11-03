//! Optional local latency summaries. Durations never cross machine clocks.

use std::time::{Duration, Instant};

const INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Default)]
struct Samples {
    count: u64,
    total: Duration,
    max: Duration,
}

/// Fixed-size counters, enabled by callers only when diagnostics are requested.
/// Reports contain timing and counts, never input contents.
pub struct LatencyStats<const N: usize> {
    labels: [&'static str; N],
    samples: [Samples; N],
    started: Instant,
}

impl<const N: usize> LatencyStats<N> {
    pub fn new(labels: [&'static str; N]) -> Self {
        Self {
            labels,
            samples: [Samples::default(); N],
            started: Instant::now(),
        }
    }

    pub fn record(&mut self, stage: usize, elapsed: Duration) {
        let sample = &mut self.samples[stage];
        sample.count += 1;
        sample.total += elapsed;
        sample.max = sample.max.max(elapsed);
    }

    pub fn report_if_due(&mut self) {
        if self.started.elapsed() >= INTERVAL {
            self.report();
        }
    }

    fn report(&mut self) {
        for (label, sample) in self.labels.iter().zip(&self.samples) {
            if sample.count != 0 {
                eprintln!(
                    "[latency] {label}: n={} mean_us={:.1} max_us={:.1}",
                    sample.count,
                    sample.total.as_secs_f64() * 1e6 / sample.count as f64,
                    sample.max.as_secs_f64() * 1e6,
                );
            }
        }
        self.samples.fill(Samples::default());
        self.started = Instant::now();
    }
}

impl<const N: usize> Drop for LatencyStats<N> {
    fn drop(&mut self) {
        self.report();
    }
}
