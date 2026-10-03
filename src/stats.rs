//! Processor, memory, network and disk readings from the operating system.

use std::time::Instant;

use sysinfo::{CpuRefreshKind, Disks, MemoryRefreshKind, Networks, RefreshKind, System};

pub struct Sample {
    /// Processor load from 0.0 to 1.0.
    pub cpu: f32,
    /// Average reported processor clock in MHz. Windows often reports the base clock.
    pub cpu_mhz: u64,
    pub ram_used: u64,
    pub ram_total: u64,
    /// Bytes per second, summed over physical network adapters.
    pub net_down: f64,
    pub net_up: f64,
    /// Used and total bytes on the system drive.
    pub disk: Option<(u64, u64)>,
    pub uptime_secs: u64,
}

pub struct Stats {
    system: System,
    networks: Networks,
    disks: Disks,
    last_sample: Instant,
}

impl Stats {
    pub fn new() -> Self {
        // CPU usage and network speed are differences between two readings, so take the first ones now.
        let system = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(MemoryRefreshKind::everything()),
        );
        Self {
            system,
            networks: Networks::new_with_refreshed_list(),
            disks: Disks::new_with_refreshed_list(),
            last_sample: Instant::now(),
        }
    }

    /// Short processor name, e.g. "Ryzen 7 7800X3D".
    pub fn cpu_name(&self) -> String {
        let brand = self.system.cpus().first().map(|c| c.brand()).unwrap_or_default();
        // Drop anything after an "@" (Intel's clock suffix), then the maker, "8-Core" and the like.
        let brand = brand.split('@').next().unwrap_or_default();
        brand
            .replace("(R)", "")
            .replace("(TM)", "")
            .split_whitespace()
            .filter(|word| {
                !word.ends_with("-Core")
                    && !["AMD", "Intel", "Core", "Processor", "CPU"].contains(word)
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Installed memory in whole gigabytes. Windows reports slightly less than what's
    /// installed (some is reserved for hardware), so round up.
    pub fn ram_total_gb(&self) -> f64 {
        (self.system.total_memory() as f64 / (1024.0 * 1024.0 * 1024.0)).ceil()
    }

    pub fn sample(&mut self) -> Sample {
        // One combined refresh: on Windows each refresh restarts the usage measurement,
        // so refreshing usage and frequency separately would read near-zero idle time.
        self.system
            .refresh_cpu_specifics(CpuRefreshKind::nothing().with_cpu_usage().with_frequency());
        self.system.refresh_memory();
        self.networks.refresh(true);
        self.disks.refresh(false);

        let elapsed = self.last_sample.elapsed().as_secs_f64().max(0.001);
        self.last_sample = Instant::now();
        let (mut down, mut up) = (0u64, 0u64);
        for (name, data) in &self.networks {
            if is_physical_adapter(name) {
                down += data.received();
                up += data.transmitted();
            }
        }

        let cpus = self.system.cpus();
        let cpu_mhz = if cpus.is_empty() {
            0
        } else {
            cpus.iter().map(|c| c.frequency()).sum::<u64>() / cpus.len() as u64
        };

        let system_drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        let disk = self
            .disks
            .iter()
            .find(|d| d.mount_point().to_string_lossy().trim_end_matches('\\') == system_drive)
            .or_else(|| self.disks.iter().next())
            .map(|d| (d.total_space() - d.available_space(), d.total_space()));

        Sample {
            cpu: (self.system.global_cpu_usage() / 100.0).clamp(0.0, 1.0),
            cpu_mhz,
            ram_used: self.system.used_memory(),
            ram_total: self.system.total_memory(),
            net_down: down as f64 / elapsed,
            net_up: up as f64 / elapsed,
            disk,
            uptime_secs: System::uptime(),
        }
    }
}

/// Virtual adapters (Hyper-V, WSL, VPNs) repeat traffic that already went through a real one.
fn is_physical_adapter(name: &str) -> bool {
    let name = name.to_lowercase();
    !["loopback", "vethernet", "virtual", "isatap", "teredo", "vpn", "tap-", "wsl"]
        .iter()
        .any(|word| name.contains(word))
}

pub fn format_uptime(secs: u64) -> String {
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let minutes = (secs % 3_600) / 60;
    match (days, hours) {
        (0, 0) => format!("{minutes}m"),
        (0, _) => format!("{hours}h {minutes}m"),
        _ => format!("{days}d {hours}h"),
    }
}
