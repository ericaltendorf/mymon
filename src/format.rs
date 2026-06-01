//! Tiny humanizing helpers (bytes, throughput, durations) for titles and,
//! later, the network/disk/process panels. Not all are wired up yet.
#![allow(dead_code)]

use std::time::Duration;

use crate::metrics::GpuMetrics;

/// Extract the distinctive model-number tokens from a CPU or GPU brand string.
///
/// A token qualifies as a model-number token when it has at least three digit
/// characters and digits make up at least half of its characters. That picks
/// out things like `i7-1370P`, `5950X`, `A6000` and drops vendor names,
/// marketing words, generation ordinals (`13th`), core counts (`16-Core`) and
/// frequency suffixes (`2.50GHz`). If nothing qualifies the input is returned
/// trimmed, so unusual brands degrade gracefully.
pub fn model_number(brand: &str) -> String {
    let kept: Vec<&str> = brand
        .split_whitespace()
        .filter(|tok| is_model_token(tok))
        .collect();
    if kept.is_empty() {
        brand.trim().to_string()
    } else {
        kept.join(" ")
    }
}

fn is_model_token(tok: &str) -> bool {
    let digits = tok.chars().filter(|c| c.is_ascii_digit()).count();
    let total = tok.chars().count();
    digits >= 3 && digits * 2 >= total
}

/// Group GPUs by extracted model and format like `"A6000x4"` or
/// `"A6000x2 + 4090"`. Empty when there are no GPUs.
pub fn gpu_summary(gpus: &[GpuMetrics]) -> String {
    let mut groups: Vec<(String, usize)> = Vec::new();
    for gpu in gpus {
        let model = model_number(&gpu.name);
        let same = groups.last().is_some_and(|(m, _)| m == &model);
        if same {
            groups.last_mut().unwrap().1 += 1;
        } else {
            groups.push((model, 1));
        }
    }
    groups
        .iter()
        .map(|(m, n)| if *n > 1 { format!("{m}x{n}") } else { m.clone() })
        .collect::<Vec<_>>()
        .join(" + ")
}

/// Compact byte count rendered as `(number, unit)` for callers that want to
/// style the unit suffix separately (e.g. dim gray "M" next to a normal-color
/// "8.2"). Uses 2-3 significant digits: `8.2M`, `73.1M`, `213M`, `18.2G`.
pub fn bytes_short(n: u64) -> (String, &'static str) {
    const UNITS: [&str; 6] = ["B", "K", "M", "G", "T", "P"];
    let mut v = n as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    let num = if v >= 100.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    };
    (num, UNITS[unit])
}

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
