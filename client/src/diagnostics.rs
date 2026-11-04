//! Per-input wall-clock diagnostics, enabled only by --latency-stats.

use std::cell::Cell;
use std::time::{Duration, Instant};

const SLOW: Duration = Duration::from_millis(2);
const LABELS: [&str; 6] = ["bounds", "state", "create", "configure", "post", "drop"];

#[derive(Clone, Copy)]
pub enum Stage {
    Bounds,
    State,
    Create,
    Configure,
    Post,
    Drop,
}

#[derive(Clone, Copy)]
struct Sample {
    sequence: u64,
    kind: &'static str,
    started: Instant,
    stages: [Duration; 6],
}

thread_local! {
    static ACTIVE: Cell<Option<Sample>> = const { Cell::new(None) };
    static SEQUENCE: Cell<u64> = const { Cell::new(0) };
}

pub struct Injection;

impl Injection {
    pub fn begin(enabled: bool, kind: &'static str) -> Self {
        if enabled {
            let sequence = SEQUENCE.with(|value| {
                let next = value.get().wrapping_add(1);
                value.set(next);
                next
            });
            ACTIVE.set(Some(Sample {
                sequence,
                kind,
                started: Instant::now(),
                stages: [Duration::ZERO; 6],
            }));
        }
        Self
    }
}

impl Drop for Injection {
    fn drop(&mut self) {
        let Some(sample) = ACTIVE.take() else {
            return;
        };
        let elapsed = sample.started.elapsed();
        if elapsed < SLOW {
            return;
        }
        let accounted: Duration = sample.stages.iter().sum();
        eprint!(
            "[{}] [slow-inject] epoch_us={} seq={} kind={} total_us={}",
            crate::timestamp(),
            time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1000,
            sample.sequence,
            sample.kind,
            elapsed.as_micros(),
        );
        for (label, duration) in LABELS.iter().zip(sample.stages) {
            eprint!(" {label}_us={}", duration.as_micros());
        }
        eprintln!(
            " other_us={}",
            elapsed.saturating_sub(accounted).as_micros()
        );
    }
}

/// Stages must not overlap; other_us includes work outside these scopes.
pub fn measure<T>(stage: Stage, action: impl FnOnce() -> T) -> T {
    let started = ACTIVE.with(|active| active.get().map(|_| Instant::now()));
    let result = action();
    if let Some(started) = started {
        let elapsed = started.elapsed();
        ACTIVE.with(|active| {
            if let Some(mut sample) = active.get() {
                sample.stages[stage as usize] += elapsed;
                active.set(Some(sample));
            }
        });
    }
    result
}
