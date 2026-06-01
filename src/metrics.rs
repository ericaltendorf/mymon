//! Data model for a single point-in-time sample of system metrics.
//!
//! These types are deliberately decoupled from the collection logic in
//! [`crate::monitor`] and from any rendering. A renderer should be able to
//! consume a [`Snapshot`] without knowing anything about `sysinfo` or NVML.
//!
//! Conventions:
//! - All sizes are in **bytes**.
//! - All rates are in **bytes per second** (or packets per second) and are
//!   computed from the delta since the previous refresh.
//! - All temperatures are in **degrees Celsius**.
//! - All frequencies/clocks are in **MHz**.
//!
//! This is the complete polling model; the TUI currently only surfaces a
//! subset (CPU/memory/GPU). The remaining fields (network, disk, per-process
//! detail) are collected and ready for the panels we add next, hence the
//! module-wide `dead_code` allowance.
#![allow(dead_code)]

use std::time::Duration;

/// A sample of system state at one instant.
///
/// The fields fall into two groups with different refresh cadences: the cheap
/// "stats" (CPU/memory/GPU/network/disk) are refreshed frequently for smooth
/// graphs, while [`processes`](Snapshot::processes) — which requires scanning
/// all of `/proc` and is by far the most expensive part — is refreshed less
/// often. The two are updated in place by
/// [`Monitor::refresh_stats`](crate::monitor::Monitor::refresh_stats) and
/// [`Monitor::refresh_processes`](crate::monitor::Monitor::refresh_processes).
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// Wall-clock time the stats were taken (seconds since the Unix epoch).
    pub unix_time: u64,
    /// Time elapsed since the previous stats refresh; rates use this.
    pub interval: Duration,
    /// Static-ish host information.
    pub host: HostInfo,
    pub cpu: CpuMetrics,
    pub memory: MemoryMetrics,
    /// Thermal sensors exposed by the platform (not GPU sensors).
    pub temperatures: Vec<Temperature>,
    pub networks: Vec<NetworkMetrics>,
    pub disks: Vec<DiskMetrics>,
    /// NVIDIA GPUs, empty if NVML is unavailable or there are no GPUs.
    pub gpus: Vec<GpuMetrics>,
    /// Total number of processes seen at the last process refresh.
    pub process_count: usize,
    /// Per-process detail, refreshed on the slower process cadence.
    pub processes: Vec<ProcessMetrics>,
}

#[derive(Debug, Clone, Default)]
pub struct HostInfo {
    pub hostname: Option<String>,
    pub kernel_version: Option<String>,
    pub os_version: Option<String>,
    pub long_os_version: Option<String>,
    pub distribution_id: String,
    pub cpu_arch: String,
    /// System uptime.
    pub uptime: Duration,
    /// Boot time as seconds since the Unix epoch.
    pub boot_time: u64,
}

#[derive(Debug, Clone, Default)]
pub struct CpuMetrics {
    /// Aggregate usage across all logical cores, 0.0..=100.0.
    pub global_usage: f32,
    pub per_core: Vec<CoreMetrics>,
    pub physical_cores: Option<usize>,
    pub brand: String,
    pub vendor_id: String,
    /// 1, 5 and 15 minute load averages.
    pub load_average: LoadAverage,
}

impl CpuMetrics {
    /// Number of logical cores.
    pub fn logical_cores(&self) -> usize {
        self.per_core.len()
    }
}

#[derive(Debug, Clone, Default)]
pub struct CoreMetrics {
    pub name: String,
    /// Usage 0.0..=100.0.
    pub usage: f32,
    /// Current frequency in MHz (0 if unknown).
    pub frequency_mhz: u64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct LoadAverage {
    pub one: f64,
    pub five: f64,
    pub fifteen: f64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MemoryMetrics {
    pub total: u64,
    pub used: u64,
    pub available: u64,
    pub free: u64,
    pub swap_total: u64,
    pub swap_used: u64,
    pub swap_free: u64,
}

impl MemoryMetrics {
    pub fn used_fraction(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.used as f64 / self.total as f64
        }
    }

    pub fn swap_used_fraction(&self) -> f64 {
        if self.swap_total == 0 {
            0.0
        } else {
            self.swap_used as f64 / self.swap_total as f64
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Temperature {
    pub label: String,
    pub celsius: Option<f32>,
    pub max: Option<f32>,
    pub critical: Option<f32>,
}

#[derive(Debug, Clone, Default)]
pub struct NetworkMetrics {
    pub interface: String,
    /// Receive throughput in bytes/second over the last interval.
    pub rx_rate: f64,
    /// Transmit throughput in bytes/second over the last interval.
    pub tx_rate: f64,
    pub rx_packets_rate: f64,
    pub tx_packets_rate: f64,
    /// Cumulative bytes received since the interface (or program) started.
    pub total_received: u64,
    pub total_transmitted: u64,
    pub total_errors_rx: u64,
    pub total_errors_tx: u64,
    pub mac_address: String,
    pub ip_networks: Vec<String>,
    pub mtu: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskKind {
    Hdd,
    Ssd,
    Unknown,
}

impl Default for DiskKind {
    fn default() -> Self {
        DiskKind::Unknown
    }
}

#[derive(Debug, Clone, Default)]
pub struct DiskMetrics {
    pub name: String,
    pub mount_point: String,
    pub file_system: String,
    pub kind: DiskKind,
    pub total_space: u64,
    pub available_space: u64,
    pub used_space: u64,
    /// Read throughput in bytes/second over the last interval.
    pub read_rate: f64,
    /// Write throughput in bytes/second over the last interval.
    pub write_rate: f64,
    pub total_read_bytes: u64,
    pub total_written_bytes: u64,
    pub is_removable: bool,
    pub is_read_only: bool,
}

impl DiskMetrics {
    pub fn used_fraction(&self) -> f64 {
        if self.total_space == 0 {
            0.0
        } else {
            self.used_space as f64 / self.total_space as f64
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct GpuMetrics {
    pub index: u32,
    pub name: String,
    pub uuid: String,
    /// GPU core utilization, 0..=100.
    pub utilization_gpu: Option<u32>,
    /// Memory bus utilization, 0..=100.
    pub utilization_memory: Option<u32>,
    /// Video encoder utilization, 0..=100.
    pub encoder_utilization: Option<u32>,
    pub memory_total: u64,
    pub memory_used: u64,
    pub memory_free: u64,
    pub temperature_c: Option<u32>,
    pub power_usage_w: Option<f64>,
    pub power_limit_w: Option<f64>,
    /// Fan speed as a percentage of maximum, 0..=100.
    pub fan_speed_percent: Option<u32>,
    pub clock_graphics_mhz: Option<u32>,
    pub clock_sm_mhz: Option<u32>,
    pub clock_memory_mhz: Option<u32>,
    pub processes: Vec<GpuProcess>,
}

impl GpuMetrics {
    pub fn memory_used_fraction(&self) -> f64 {
        if self.memory_total == 0 {
            0.0
        } else {
            self.memory_used as f64 / self.memory_total as f64
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct GpuProcess {
    pub pid: u32,
    /// GPU memory used by this process in bytes, if known.
    pub used_memory: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct ProcessMetrics {
    pub pid: u32,
    pub parent_pid: Option<u32>,
    pub name: String,
    pub command: Vec<String>,
    pub exe: Option<String>,
    pub user: Option<String>,
    /// CPU usage summed across cores; can exceed 100.0 on multi-threaded
    /// processes (e.g. up to `logical_cores * 100`), matching htop's default.
    pub cpu_usage: f32,
    /// Resident set size in bytes.
    pub memory: u64,
    /// Virtual memory size in bytes.
    pub virtual_memory: u64,
    pub status: String,
    /// Time the process has been running, in seconds.
    pub run_time: u64,
    /// Start time as seconds since the Unix epoch.
    pub start_time: u64,
    pub disk_read_rate: f64,
    pub disk_write_rate: f64,
    /// GPU memory used by this process across all GPUs, if it is on a GPU.
    pub gpu_memory: Option<u64>,
}
