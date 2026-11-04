//! Passive benchmark of the same display cache used by the input client.
//! No events are created or posted. Display changes are reported as snapshots.

#[path = "../src/display_cache.rs"]
mod display_cache;

use std::hint::black_box;
use std::time::{Duration, Instant};

fn timestamp() -> String {
    time::OffsetDateTime::now_utc().unix_timestamp().to_string()
}

fn main() -> std::io::Result<()> {
    let cache = display_cache::DisplayCache::main_display()?;
    let mut previous = cache.snapshot();
    println!("initial: {previous:?}");
    let mut total = Duration::ZERO;
    let mut maximum = Duration::ZERO;
    let count = 2500;
    for _ in 0..count {
        let started = Instant::now();
        let snapshot = black_box(cache.snapshot());
        let elapsed = started.elapsed();
        total += elapsed;
        maximum = maximum.max(elapsed);
        if snapshot != previous {
            println!("changed: {snapshot:?}");
            previous = snapshot;
        }
        std::thread::sleep(Duration::from_millis(8));
    }
    println!(
        "cached bounds: n={count} mean_us={:.3} max_us={:.3}",
        total.as_secs_f64() * 1e6 / f64::from(count),
        maximum.as_secs_f64() * 1e6,
    );
    Ok(())
}
