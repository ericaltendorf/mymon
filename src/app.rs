//! Application state: owns the [`Monitor`], the latest [`Snapshot`], and the
//! rolling histories that feed the time-series graphs.
//!
//! Collection runs on two independent cadences so the app sleeps most of the
//! time: cheap stats (CPU/memory/GPU/network/disk) refresh on `stats_interval`
//! for smooth graphs, while the expensive process scan refreshes on the slower
//! `process_interval`.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::metrics::Snapshot;
use crate::monitor::Monitor;

/// Maximum number of historical samples kept per series. This comfortably
/// covers very wide terminals (graph width is `2 * columns` dots).
const HISTORY_CAP: usize = 2048;

/// A fixed-capacity rolling buffer of `f64` samples (oldest..newest).
#[derive(Default)]
pub struct History {
    buf: VecDeque<f64>,
}

impl History {
    pub fn push(&mut self, v: f64) {
        if self.buf.len() == HISTORY_CAP {
            self.buf.pop_front();
        }
        self.buf.push_back(v);
    }

    /// Samples as a contiguous slice-backed `Vec`, oldest first.
    pub fn to_vec(&self) -> Vec<f64> {
        self.buf.iter().copied().collect()
    }
}

pub struct App {
    monitor: Monitor,
    pub snapshot: Snapshot,
    /// False until the first stats sample lands.
    pub ready: bool,
    pub cpu_history: History,
    pub mem_history: History,
    pub gpu_history: History,
    pub gpu_mem_history: History,
    pub stats_interval: Duration,
    pub process_interval: Duration,
    last_stats: Instant,
    last_process: Instant,
    pub should_quit: bool,
}

impl App {
    pub fn new(stats_interval: Duration, process_interval: Duration) -> Self {
        let mut app = App {
            monitor: Monitor::new(),
            snapshot: Snapshot::default(),
            ready: false,
            cpu_history: History::default(),
            mem_history: History::default(),
            gpu_history: History::default(),
            gpu_mem_history: History::default(),
            stats_interval,
            process_interval,
            last_stats: Instant::now(),
            last_process: Instant::now(),
            should_quit: false,
        };
        // Prime once so the first frame has data (processes included).
        app.on_stats_tick();
        app.on_process_tick();
        app
    }

    /// Refresh the cheap stats and append to the histories.
    pub fn on_stats_tick(&mut self) {
        self.monitor.refresh_stats(&mut self.snapshot);
        self.cpu_history.push(self.snapshot.cpu.global_usage as f64);
        self.mem_history
            .push(self.snapshot.memory.used_fraction() * 100.0);
        self.gpu_history.push(mean_gpu_util(&self.snapshot));
        self.gpu_mem_history
            .push(mean_gpu_mem(&self.snapshot) * 100.0);
        self.ready = true;
        self.last_stats = Instant::now();
    }

    /// Refresh the (expensive) process table.
    pub fn on_process_tick(&mut self) {
        self.monitor.refresh_processes(&mut self.snapshot);
        self.last_process = Instant::now();
    }

    /// Run any due ticks. Returns true if anything was refreshed (redraw hint).
    pub fn update(&mut self) -> bool {
        let now = Instant::now();
        let mut refreshed = false;
        if now.duration_since(self.last_stats) >= self.stats_interval {
            self.on_stats_tick();
            refreshed = true;
        }
        if now.duration_since(self.last_process) >= self.process_interval {
            self.on_process_tick();
            refreshed = true;
        }
        refreshed
    }

    /// How long the event loop may block before the next tick is due.
    pub fn time_until_next_tick(&self) -> Duration {
        let now = Instant::now();
        let until_stats = self
            .stats_interval
            .saturating_sub(now.duration_since(self.last_stats));
        let until_process = self
            .process_interval
            .saturating_sub(now.duration_since(self.last_process));
        until_stats.min(until_process)
    }
}

/// Mean GPU core utilization across all GPUs (0..=100), or 0 if none report.
pub fn mean_gpu_util(snap: &Snapshot) -> f64 {
    let utils: Vec<f64> = snap
        .gpus
        .iter()
        .filter_map(|g| g.utilization_gpu.map(|u| u as f64))
        .collect();
    if utils.is_empty() {
        0.0
    } else {
        utils.iter().sum::<f64>() / utils.len() as f64
    }
}

/// Mean GPU memory used fraction across all GPUs (0.0..=1.0), or 0 if none.
pub fn mean_gpu_mem(snap: &Snapshot) -> f64 {
    if snap.gpus.is_empty() {
        return 0.0;
    }
    let sum: f64 = snap.gpus.iter().map(|g| g.memory_used_fraction()).sum();
    sum / snap.gpus.len() as f64
}
