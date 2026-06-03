//! System metric collection.
//!
//! [`Monitor`] owns the long-lived OS handles (sysinfo's `System`, `Networks`,
//! `Disks`, ... and an optional NVML handle) and turns each [`Monitor::refresh`]
//! call into an immutable [`Snapshot`]. Rates (network/disk throughput) are
//! derived from the wall-clock interval between successive refreshes.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sysinfo::{
    Components, DiskKind as SysDiskKind, DiskRefreshKind, Disks, MINIMUM_CPU_UPDATE_INTERVAL,
    Networks, ProcessRefreshKind, ProcessesToUpdate, System, Users,
};

use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};
use nvml_wrapper::enums::device::UsedGpuMemory;

use crate::metrics::*;

/// Long-lived collector. Construct once with [`Monitor::new`], then call
/// [`Monitor::refresh`] on a cadence to obtain [`Snapshot`]s.
pub struct Monitor {
    system: System,
    networks: Networks,
    disks: Disks,
    components: Components,
    users: Users,
    nvml: Option<Nvml>,
    last_refresh: Instant,
    /// Hostname/kernel/OS strings + boot time — these don't change at runtime,
    /// so we read them once and clone into each snapshot.
    static_host: HostInfo,
}

impl Monitor {
    /// Initialize all collectors. NVML is optional: if the driver/library is
    /// missing this still succeeds and simply reports no GPUs.
    pub fn new() -> Self {
        let mut system = System::new_all();
        // Prime CPU counters so the first refresh produces meaningful deltas.
        system.refresh_cpu_all();

        let networks = Networks::new_with_refreshed_list();
        let disks = Disks::new_with_refreshed_list_specifics(
            DiskRefreshKind::everything().with_io_usage(),
        );
        let components = Components::new_with_refreshed_list();
        let users = Users::new_with_refreshed_list();

        let nvml = match Nvml::init() {
            Ok(nvml) => Some(nvml),
            Err(e) => {
                // Not fatal: most machines won't have an NVIDIA GPU/driver.
                eprintln!("note: NVIDIA GPU monitoring unavailable: {e}");
                None
            }
        };

        let static_host = HostInfo {
            hostname: System::host_name(),
            kernel_version: System::kernel_version(),
            os_version: System::os_version(),
            long_os_version: System::long_os_version(),
            distribution_id: System::distribution_id(),
            cpu_arch: System::cpu_arch(),
            uptime: Duration::ZERO,
            boot_time: System::boot_time(),
        };

        Monitor {
            system,
            networks,
            disks,
            components,
            users,
            nvml,
            last_refresh: Instant::now(),
            static_host,
        }
    }

    /// Refresh every subsystem and produce a fresh [`Snapshot`].
    ///
    /// Refresh the cheap "stats" subsystems (CPU, memory, GPU, network, disk,
    /// thermals) and update them in `snap` in place. Leaves
    /// [`Snapshot::processes`] untouched.
    ///
    /// Only the subsystems the UI actually shows (CPU, memory, GPU) are
    /// refreshed here — reading thermal sensors (`hwmon`) and `statvfs`-ing every
    /// mount were the bulk of the cost and aren't displayed yet. Call
    /// [`refresh_peripherals`](Self::refresh_peripherals) when a network/disk/
    /// temperature panel needs them.
    ///
    /// For accurate CPU and rate figures, call this no faster than
    /// [`sysinfo::MINIMUM_CPU_UPDATE_INTERVAL`] apart.
    pub fn refresh_stats(&mut self, snap: &mut Snapshot) {
        let now = Instant::now();
        let interval = now.duration_since(self.last_refresh);
        self.last_refresh = now;

        self.system.refresh_cpu_all();
        self.system.refresh_memory();

        snap.unix_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        snap.interval = interval;
        snap.host = self.collect_host();
        snap.cpu = self.collect_cpu();
        snap.memory = self.collect_memory();
        snap.gpus = self.collect_gpus();
    }

    /// Refresh the peripherals the hot path skips (network, disk, thermals) and
    /// update them in `snap`. Comparatively expensive (`hwmon` + `statvfs`), so
    /// call this only when a panel that needs them is on screen.
    #[allow(dead_code)] // wired up when network/disk/temperature panels land
    pub fn refresh_peripherals(&mut self, snap: &mut Snapshot) {
        let secs = snap.interval.as_secs_f64().max(1e-3);
        self.networks.refresh(true);
        self.disks
            .refresh_specifics(false, DiskRefreshKind::everything().with_io_usage());
        self.components.refresh(false);
        snap.temperatures = self.collect_temperatures();
        snap.networks = self.collect_networks(secs);
        snap.disks = self.collect_disks(secs);
    }

    /// Refresh disk free-space and return any "real" mount that's running
    /// low: free space below `threshold` bytes AND below 5% of total. The
    /// percentage gate keeps small special-purpose partitions like `/boot`
    /// from firing whenever they happen to sit below the absolute limit —
    /// only mounts that are both absolutely and proportionally tight fire.
    /// Skips pseudo-filesystems (tmpfs, squashfs, overlay, …), well-known
    /// system mounts (`/proc`, `/sys`, `/dev`, `/run`, `/snap`, `/boot/efi`),
    /// and partitions smaller than 1 GiB.
    pub fn refresh_disk_warnings(&mut self, threshold: u64) -> Vec<(String, u64)> {
        self.disks
            .refresh_specifics(true, DiskRefreshKind::nothing().with_storage());
        self.disks
            .list()
            .iter()
            .filter(|d| is_real_mount(d))
            .filter(|d| {
                let free = d.available_space();
                let total = d.total_space();
                free < threshold && total > 0 && (free as f64) < 0.05 * total as f64
            })
            .map(|d| {
                (
                    d.mount_point().to_string_lossy().into_owned(),
                    d.available_space(),
                )
            })
            .collect()
    }

    /// Send SIGTERM to `pid` via sysinfo's per-process handle. Returns true if
    /// the signal was delivered; false if the process is unknown to sysinfo or
    /// the kernel refused (typically EPERM). The next `refresh_processes` will
    /// pick up whether the process actually exited.
    pub fn kill(&self, pid: u32) -> bool {
        self.system
            .process(sysinfo::Pid::from_u32(pid))
            .and_then(|p| p.kill_with(sysinfo::Signal::Term))
            .unwrap_or(false)
    }

    /// Refresh the process table (the expensive part: a full `/proc` scan) and
    /// rebuild [`Snapshot::processes`] in place. Uses `snap.gpus` to attribute
    /// GPU memory to processes, so call [`refresh_stats`](Self::refresh_stats)
    /// first when you want fresh GPU figures.
    pub fn refresh_processes(&mut self, snap: &mut Snapshot) {
        // `nothing()` leaves `tasks: true`, which makes sysinfo scan every
        // `/proc/<pid>/task/<tid>` directory and count each thread as its own
        // process. On thread-heavy machines that explodes the scan cost; we
        // only care about real processes for the table, so skip tasks.
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing()
                .with_memory()
                .with_cpu()
                .with_user(sysinfo::UpdateKind::OnlyIfNotSet)
                .without_tasks(),
        );

        // Map pid -> GPU memory so per-process rows can show GPU usage.
        let mut gpu_mem_by_pid: HashMap<u32, u64> = HashMap::new();
        for gpu in &snap.gpus {
            for p in &gpu.processes {
                if let Some(mem) = p.used_memory {
                    *gpu_mem_by_pid.entry(p.pid).or_insert(0) += mem;
                }
            }
        }

        snap.processes = self.collect_processes(&gpu_mem_by_pid);
        snap.process_count = snap.processes.len();
    }

    fn collect_host(&self) -> HostInfo {
        let mut h = self.static_host.clone();
        h.uptime = Duration::from_secs(System::uptime());
        h
    }

    fn collect_cpu(&self) -> CpuMetrics {
        let cpus = self.system.cpus();
        let per_core = cpus
            .iter()
            .map(|c| CoreMetrics {
                name: c.name().to_string(),
                usage: c.cpu_usage(),
                frequency_mhz: c.frequency(),
            })
            .collect();

        let (brand, vendor_id) = cpus
            .first()
            .map(|c| (c.brand().to_string(), c.vendor_id().to_string()))
            .unwrap_or_default();

        let load = System::load_average();

        CpuMetrics {
            global_usage: self.system.global_cpu_usage(),
            per_core,
            physical_cores: System::physical_core_count(),
            brand,
            vendor_id,
            load_average: LoadAverage {
                one: load.one,
                five: load.five,
                fifteen: load.fifteen,
            },
        }
    }

    fn collect_memory(&self) -> MemoryMetrics {
        let total = self.system.total_memory();
        let used = self.system.used_memory();
        let swap_total = self.system.total_swap();
        let swap_used = self.system.used_swap();
        MemoryMetrics {
            total,
            used,
            available: self.system.available_memory(),
            free: self.system.free_memory(),
            swap_total,
            swap_used,
            swap_free: swap_total.saturating_sub(swap_used),
        }
    }

    fn collect_temperatures(&self) -> Vec<Temperature> {
        self.components
            .list()
            .iter()
            .map(|c| Temperature {
                label: c.label().to_string(),
                celsius: c.temperature(),
                max: c.max(),
                critical: c.critical(),
            })
            .collect()
    }

    fn collect_networks(&self, secs: f64) -> Vec<NetworkMetrics> {
        let mut nets: Vec<NetworkMetrics> = self
            .networks
            .list()
            .iter()
            .map(|(name, data)| NetworkMetrics {
                interface: name.clone(),
                rx_rate: data.received() as f64 / secs,
                tx_rate: data.transmitted() as f64 / secs,
                rx_packets_rate: data.packets_received() as f64 / secs,
                tx_packets_rate: data.packets_transmitted() as f64 / secs,
                total_received: data.total_received(),
                total_transmitted: data.total_transmitted(),
                total_errors_rx: data.total_errors_on_received(),
                total_errors_tx: data.total_errors_on_transmitted(),
                mac_address: data.mac_address().to_string(),
                ip_networks: data
                    .ip_networks()
                    .iter()
                    .map(|ip| format!("{}/{}", ip.addr, ip.prefix))
                    .collect(),
                mtu: data.mtu(),
            })
            .collect();
        nets.sort_by(|a, b| a.interface.cmp(&b.interface));
        nets
    }

    fn collect_disks(&self, secs: f64) -> Vec<DiskMetrics> {
        let mut disks: Vec<DiskMetrics> = self
            .disks
            .list()
            .iter()
            .map(|d| {
                let usage = d.usage();
                let total = d.total_space();
                let available = d.available_space();
                DiskMetrics {
                    name: d.name().to_string_lossy().into_owned(),
                    mount_point: d.mount_point().to_string_lossy().into_owned(),
                    file_system: d.file_system().to_string_lossy().into_owned(),
                    kind: match d.kind() {
                        SysDiskKind::HDD => DiskKind::Hdd,
                        SysDiskKind::SSD => DiskKind::Ssd,
                        SysDiskKind::Unknown(_) => DiskKind::Unknown,
                    },
                    total_space: total,
                    available_space: available,
                    used_space: total.saturating_sub(available),
                    read_rate: usage.read_bytes as f64 / secs,
                    write_rate: usage.written_bytes as f64 / secs,
                    total_read_bytes: usage.total_read_bytes,
                    total_written_bytes: usage.total_written_bytes,
                    is_removable: d.is_removable(),
                    is_read_only: d.is_read_only(),
                }
            })
            .collect();
        disks.sort_by(|a, b| a.mount_point.cmp(&b.mount_point));
        disks
    }

    fn collect_gpus(&self) -> Vec<GpuMetrics> {
        let Some(nvml) = &self.nvml else {
            return Vec::new();
        };
        let count = match nvml.device_count() {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

        let mut gpus = Vec::with_capacity(count as usize);
        for index in 0..count {
            let Ok(device) = nvml.device_by_index(index) else {
                continue;
            };

            let util = device.utilization_rates().ok();
            let mem = device.memory_info().ok();

            let mut processes: Vec<GpuProcess> = Vec::new();
            for list in [
                device.running_compute_processes().ok(),
                device.running_graphics_processes().ok(),
            ]
            .into_iter()
            .flatten()
            {
                for p in list {
                    processes.push(GpuProcess {
                        pid: p.pid,
                        used_memory: match p.used_gpu_memory {
                            UsedGpuMemory::Used(bytes) => Some(bytes),
                            UsedGpuMemory::Unavailable => None,
                        },
                    });
                }
            }

            gpus.push(GpuMetrics {
                index,
                name: device.name().unwrap_or_else(|_| format!("GPU {index}")),
                uuid: device.uuid().unwrap_or_default(),
                utilization_gpu: util.as_ref().map(|u| u.gpu),
                utilization_memory: util.as_ref().map(|u| u.memory),
                encoder_utilization: device.encoder_utilization().ok().map(|u| u.utilization),
                memory_total: mem.as_ref().map(|m| m.total).unwrap_or(0),
                memory_used: mem.as_ref().map(|m| m.used).unwrap_or(0),
                memory_free: mem.as_ref().map(|m| m.free).unwrap_or(0),
                temperature_c: device.temperature(TemperatureSensor::Gpu).ok(),
                power_usage_w: device.power_usage().ok().map(|mw| mw as f64 / 1000.0),
                power_limit_w: device
                    .enforced_power_limit()
                    .ok()
                    .map(|mw| mw as f64 / 1000.0),
                fan_speed_percent: device.fan_speed(0).ok(),
                clock_graphics_mhz: device.clock_info(Clock::Graphics).ok(),
                clock_sm_mhz: device.clock_info(Clock::SM).ok(),
                clock_memory_mhz: device.clock_info(Clock::Memory).ok(),
                processes,
            });
        }
        gpus
    }

    fn collect_processes(&self, gpu_mem_by_pid: &HashMap<u32, u64>) -> Vec<ProcessMetrics> {
        // We only refresh cpu/memory/user for processes, so `command`, `exe`
        // and per-process disk I/O are intentionally left empty here to keep the
        // (already infrequent) process scan cheap.
        self.system
            .processes()
            .values()
            .map(|p| {
                let pid = p.pid().as_u32();
                let user = p
                    .user_id()
                    .and_then(|uid| self.users.get_user_by_id(uid))
                    .map(|u| u.name().to_string());
                ProcessMetrics {
                    pid,
                    parent_pid: p.parent().map(|pp| pp.as_u32()),
                    name: p.name().to_string_lossy().into_owned(),
                    command: Vec::new(),
                    exe: None,
                    user,
                    cpu_usage: p.cpu_usage(),
                    memory: p.memory(),
                    virtual_memory: p.virtual_memory(),
                    status: p.status().to_string(),
                    run_time: p.run_time(),
                    start_time: p.start_time(),
                    disk_read_rate: 0.0,
                    disk_write_rate: 0.0,
                    gpu_memory: gpu_mem_by_pid.get(&pid).copied(),
                }
            })
            .collect()
    }
}

impl Default for Monitor {
    fn default() -> Self {
        Self::new()
    }
}

/// Distinguish user-data filesystems from kernel and snap pseudo-filesystems.
/// Used by [`Monitor::refresh_disk_warnings`].
fn is_real_mount(d: &sysinfo::Disk) -> bool {
    let fs = d.file_system().to_string_lossy().to_lowercase();
    if matches!(
        fs.as_str(),
        "tmpfs"
            | "devtmpfs"
            | "proc"
            | "sysfs"
            | "cgroup"
            | "cgroup2"
            | "overlay"
            | "squashfs"
            | "ramfs"
            | "debugfs"
            | "tracefs"
            | "securityfs"
            | "pstore"
            | "bpf"
            | "fusectl"
            | "binfmt_misc"
            | "configfs"
            | "mqueue"
            | "hugetlbfs"
            | "autofs"
            | "efivarfs"
            | "nsfs"
    ) {
        return false;
    }
    let mp = d.mount_point().to_string_lossy();
    if mp.starts_with("/proc/")
        || mp.starts_with("/sys/")
        || mp.starts_with("/dev/")
        || mp.starts_with("/run/")
        || mp.starts_with("/snap/")
        || mp == "/boot/efi"
    {
        return false;
    }
    // Below ~1 GiB is almost certainly the ESP, /boot, or a ramdisk.
    if d.total_space() < 1024 * 1024 * 1024 {
        return false;
    }
    true
}

/// Convenience re-export so callers can sleep the recommended minimum between
/// refreshes without importing `sysinfo` directly.
pub const MIN_REFRESH_INTERVAL: Duration = MINIMUM_CPU_UPDATE_INTERVAL;
