//! Tiny humanizing helpers (bytes, throughput, durations) for titles and,
//! later, the network/disk/process panels. Not all are wired up yet.
#![allow(dead_code)]

use std::time::Duration;

/// Format a byte count using binary (IEC) units: B, KiB, MiB, ...
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut v = n as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

/// Format a throughput in bytes/second as e.g. `1.2 MiB/s`.
pub fn rate(bytes_per_sec: f64) -> String {
    const UNITS: [&str; 6] = ["B/s", "KiB/s", "MiB/s", "GiB/s", "TiB/s", "PiB/s"];
    let mut v = bytes_per_sec.max(0.0);
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    format!("{v:.1} {}", UNITS[unit])
}

/// Format a duration as `Dd HH:MM:SS` (days omitted when zero).
pub fn duration(d: Duration) -> String {
    let total = d.as_secs();
    let days = total / 86_400;
    let hours = (total % 86_400) / 3_600;
    let mins = (total % 3_600) / 60;
    let secs = total % 60;
    if days > 0 {
        format!("{days}d {hours:02}:{mins:02}:{secs:02}")
    } else {
        format!("{hours:02}:{mins:02}:{secs:02}")
    }
}
