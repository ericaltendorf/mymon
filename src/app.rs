//! Application state: owns the [`Monitor`], the latest [`Snapshot`], and the
//! rolling histories that feed the time-series graphs.
//!
//! Collection runs on two independent cadences so the app sleeps most of the
//! time: cheap stats (CPU/memory/GPU/network/disk) refresh on `stats_interval`
//! for smooth graphs, while the expensive process scan refreshes on the slower
//! `process_interval`.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::metrics::{ProcessMetrics, Snapshot};
use crate::monitor::Monitor;

/// Sort key for a process pane. Shared between the app's interaction state
/// (which pane is active, what's being killed) and the renderer.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ProcSort {
    Cpu,
    Mem,
}

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
    /// Index of the selected row in the active process pane (0 = top).
    pub selected_index: usize,
    /// Which process pane the selection lives in.
    pub active_pane: ProcSort,
    /// When `Some`, the bottom status bar is showing a kill-confirm prompt
    /// for this `(pid, command)`. `y` sends SIGTERM, anything else cancels.
    pub kill_prompt: Option<(u32, String)>,
    /// Toggled by `h` / `?`. When true (and no kill prompt is active) the
    /// bottom status bar renders a key-hints line; otherwise the status row
    /// is reclaimed by the process list.
    pub show_help: bool,
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
            selected_index: 0,
            active_pane: ProcSort::Cpu,
            kill_prompt: None,
            show_help: false,
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

    /// Move the selection up (`delta < 0`) or down (`delta > 0`). Clamped to
    /// the current process count so the user can't run off the end.
    pub fn move_selection(&mut self, delta: i32) {
        let count = self.snapshot.processes.len();
        if count == 0 {
            self.selected_index = 0;
            return;
        }
        let new_idx = if delta < 0 {
            self.selected_index.saturating_sub((-delta) as usize)
        } else {
            self.selected_index
                .saturating_add(delta as usize)
                .min(count - 1)
        };
        self.selected_index = new_idx;
    }

    /// Switch which process pane the selection lives in (only meaningful in
    /// dual-pane mode; cheap no-op in single-pane).
    pub fn toggle_pane(&mut self) {
        self.active_pane = match self.active_pane {
            ProcSort::Cpu => ProcSort::Mem,
            ProcSort::Mem => ProcSort::Cpu,
        };
    }

    /// Look up the process at the active pane's selected index and arm the
    /// kill-confirm prompt.
    pub fn request_kill_selected(&mut self) {
        let mut procs: Vec<&ProcessMetrics> = self.snapshot.processes.iter().collect();
        match self.active_pane {
            ProcSort::Cpu => procs.sort_by(|a, b| {
                b.cpu_usage
                    .partial_cmp(&a.cpu_usage)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }),
            ProcSort::Mem => procs.sort_by(|a, b| b.memory.cmp(&a.memory)),
        }
        if let Some(p) = procs.get(self.selected_index) {
            self.kill_prompt = Some((p.pid, p.name.clone()));
        }
    }

    /// Send SIGTERM to the pending kill target and clear the prompt.
    pub fn confirm_kill(&mut self) {
        if let Some((pid, _)) = self.kill_prompt.take() {
            let _ = self.monitor.kill(pid);
        }
    }

    /// Drop the kill-confirm prompt without sending a signal.
    pub fn cancel_kill(&mut self) {
        self.kill_prompt = None;
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
