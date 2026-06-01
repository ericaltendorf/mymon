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

        Monitor {
            system,
            networks,
            disks,
            components,
            users,
            nvml,
            last_refresh: Instant::now(),
        }
    }

    /// Refresh every subsystem and produce a fresh [`Snapshot`].
    ///
    /// For accurate CPU and rate figures, call this no faster than
    /// [`sysinfo::MINIMUM_CPU_UPDATE_INTERVAL`] apart.
    pub fn refresh(&mut self) -> Snapshot {
        let now = Instant::now();
        let interval = now.duration_since(self.last_refresh);
        self.last_refresh = now;
        // Avoid division by zero when computing rates on the very first tick.
        let secs = interval.as_secs_f64().max(1e-3);

        // --- CPU & memory ---
        self.system.refresh_cpu_all();
        self.system.refresh_memory();

        // --- Processes ---
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing()
                .with_memory()
                .with_cpu()
                .with_disk_usage()
                .with_user(sysinfo::UpdateKind::OnlyIfNotSet)
                .with_exe(sysinfo::UpdateKind::OnlyIfNotSet),
        );

        // --- Peripherals ---
        self.networks.refresh(true);
        self.disks
            .refresh_specifics(false, DiskRefreshKind::everything().with_io_usage());
        self.components.refresh(false);

        let host = self.collect_host();
        let cpu = self.collect_cpu();
        let memory = self.collect_memory();
        let temperatures = self.collect_temperatures();
        let networks = self.collect_networks(secs);
        let disks = self.collect_disks(secs);
        let gpus = self.collect_gpus();

        // Map pid -> GPU memory so per-process rows can show GPU usage.
        let mut gpu_mem_by_pid: HashMap<u32, u64> = HashMap::new();
        for gpu in &gpus {
            for p in &gpu.processes {
                if let Some(mem) = p.used_memory {
                    *gpu_mem_by_pid.entry(p.pid).or_insert(0) += mem;
                }
            }
        }

        let processes = self.collect_processes(secs, &gpu_mem_by_pid);

        Snapshot {
            unix_time: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            interval,
            host,
            cpu,
            memory,
            temperatures,
            networks,
            disks,
            gpus,
            processes,
        }
    }

    fn collect_host(&self) -> HostInfo {
        HostInfo {
            hostname: System::host_name(),
            kernel_version: System::kernel_version(),
            os_version: System::os_version(),
            long_os_version: System::long_os_version(),
            distribution_id: System::distribution_id(),
            cpu_arch: System::cpu_arch(),
            uptime: Duration::from_secs(System::uptime()),
            boot_time: System::boot_time(),
        }
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

    fn collect_processes(
        &self,
        secs: f64,
        gpu_mem_by_pid: &HashMap<u32, u64>,
    ) -> Vec<ProcessMetrics> {
        self.system
            .processes()
            .values()
            .map(|p| {
                let pid = p.pid().as_u32();
                let disk = p.disk_usage();
                let user = p
                    .user_id()
                    .and_then(|uid| self.users.get_user_by_id(uid))
                    .map(|u| u.name().to_string());
                ProcessMetrics {
                    pid,
                    parent_pid: p.parent().map(|pp| pp.as_u32()),
                    name: p.name().to_string_lossy().into_owned(),
                    command: p
                        .cmd()
                        .iter()
                        .map(|s| s.to_string_lossy().into_owned())
                        .collect(),
                    exe: p.exe().map(|e| e.to_string_lossy().into_owned()),
                    user,
                    cpu_usage: p.cpu_usage(),
                    memory: p.memory(),
                    virtual_memory: p.virtual_memory(),
                    status: p.status().to_string(),
                    run_time: p.run_time(),
                    start_time: p.start_time(),
                    disk_read_rate: disk.read_bytes as f64 / secs,
                    disk_write_rate: disk.written_bytes as f64 / secs,
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

/// Convenience re-export so callers can sleep the recommended minimum between
/// refreshes without importing `sysinfo` directly.
pub const MIN_REFRESH_INTERVAL: Duration = MINIMUM_CPU_UPDATE_INTERVAL;
