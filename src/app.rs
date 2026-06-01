//! Application state: owns the [`Monitor`], the latest [`Snapshot`], and the
//! rolling histories that feed the time-series graphs.

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
    pub snapshot: Option<Snapshot>,
    pub cpu_history: History,
    pub mem_history: History,
    pub gpu_history: History,
    pub tick_rate: Duration,
    pub last_tick: Instant,
    pub should_quit: bool,
}

impl App {
    pub fn new(tick_rate: Duration) -> Self {
        App {
            monitor: Monitor::new(),
            snapshot: None,
            cpu_history: History::default(),
            mem_history: History::default(),
            gpu_history: History::default(),
            tick_rate,
            last_tick: Instant::now(),
            should_quit: false,
        }
    }

    /// Sample all subsystems and append to the histories.
    pub fn on_tick(&mut self) {
        let snap = self.monitor.refresh();

        self.cpu_history.push(snap.cpu.global_usage as f64);
        self.mem_history.push(snap.memory.used_fraction() * 100.0);

        // Aggregate GPU utilization as the mean across all GPUs.
        let gpu_util = mean_gpu_util(&snap);
        self.gpu_history.push(gpu_util);

        self.snapshot = Some(snap);
        self.last_tick = Instant::now();
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
