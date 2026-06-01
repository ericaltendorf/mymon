//! Tiny humanizing helpers (bytes, throughput, durations) for titles and,
//! later, the network/disk/process panels. Not all are wired up yet.
#![allow(dead_code)]

use std::time::Duration;

/// Condense a verbose CPU brand string down to the distinctive model name.
///
/// e.g. `"13th Gen Intel(R) Core(TM) i7-1370P"` -> `"i7-1370P"`,
/// `"AMD Ryzen 9 5950X 16-Core Processor"` -> `"Ryzen 9 5950X"`,
/// `"Intel(R) Xeon(R) Gold 6248 CPU @ 2.50GHz"` -> `"Xeon Gold 6248"`.
pub fn cpu_model(brand: &str) -> String {
    let mut s = brand.to_string();
    for junk in ["(R)", "(r)", "(TM)", "(tm)", "\u{00ae}", "\u{2122}"] {
        s = s.replace(junk, " ");
    }
    // Drop a trailing APU graphics blurb and any "@ 3.50GHz" frequency.
    if let Some(i) = s.find("w/") {
        s.truncate(i);
    }
    if let Some(i) = s.find('@') {
        s.truncate(i);
    }

    const DROP: [&str; 8] = [
        "Intel",
        "AMD",
        "CPU",
        "Processor",
        "Core",
        "Gen",
        "with",
        "Technology",
    ];
    let kept: Vec<&str> = s
        .split_whitespace()
        .filter(|tok| {
            // Marketing/generation noise: "13th", "1st", "16-Core", ...
            if let Some(stem) = tok
                .strip_suffix("th")
                .or_else(|| tok.strip_suffix("st"))
                .or_else(|| tok.strip_suffix("nd"))
                .or_else(|| tok.strip_suffix("rd"))
            {
                if !stem.is_empty() && stem.chars().all(|c| c.is_ascii_digit()) {
                    return false;
                }
            }
            if tok.ends_with("-Core") {
                return false;
            }
            !DROP.contains(tok)
        })
        .collect();

    let result = kept.join(" ");
    if result.is_empty() {
        brand.trim().to_string()
    } else {
        result
    }
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
