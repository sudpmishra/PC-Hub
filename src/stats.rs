//! Processor, memory, network and disk readings from the operating system.

use std::collections::HashSet;
use std::time::Instant;

use sysinfo::{
    CpuRefreshKind, Disks, MemoryRefreshKind, Networks, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System,
};

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
    /// Bytes received and sent since the last sample.
    pub received: u64,
    pub sent: u64,
    /// The drives asked for (or the system drive), in that order.
    pub drives: Vec<DriveUsage>,
    pub uptime_secs: u64,
}

pub struct DriveUsage {
    /// "C:", or a folder path for drives mounted in one.
    pub name: String,
    pub used: u64,
    pub total: u64,
}

/// "d", "D:", "D:\" all become "D:".
fn drive_name(name: &str) -> String {
    let name = name.trim().trim_end_matches(['\\', '/']);
    if name.len() == 1 && name.chars().all(|c| c.is_ascii_alphabetic()) {
        format!("{}:", name.to_ascii_uppercase())
    } else if name.len() == 2 && name.ends_with(':') {
        name.to_ascii_uppercase()
    } else {
        name.to_string()
    }
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

    /// `drives` names the drives to report, such as "D:"; empty means the system drive.
    pub fn sample(&mut self, drives: &[String]) -> Sample {
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

        let wanted = if drives.is_empty() {
            vec![std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into())]
        } else {
            drives.to_vec()
        };
        let drives = wanted
            .iter()
            .filter_map(|name| {
                let name = drive_name(name);
                let disk = self
                    .disks
                    .iter()
                    .find(|d| drive_name(&d.mount_point().to_string_lossy()) == name)?;
                Some(DriveUsage {
                    used: disk.total_space() - disk.available_space(),
                    total: disk.total_space(),
                    name,
                })
            })
            .collect();

        Sample {
            cpu: (self.system.global_cpu_usage() / 100.0).clamp(0.0, 1.0),
            cpu_mhz,
            ram_used: self.system.used_memory(),
            ram_total: self.system.total_memory(),
            net_down: down as f64 / elapsed,
            net_up: up as f64 / elapsed,
            received: down,
            sent: up,
            drives,
            uptime_secs: System::uptime(),
        }
    }
}

/// An app's processes added together, for the processes sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct AppUsage {
    /// The program name without ".exe".
    pub name: String,
    /// Share of the whole processor, 0.0 to 1.0.
    pub cpu: f32,
    pub memory: u64,
    pub processes: usize,
}

impl Stats {
    fn refresh_processes(&mut self) {
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_cpu().with_memory(),
        );
    }

    /// The apps using the most processor, or memory with `by_memory`, with all of each
    /// app's processes (browsers run dozens) added together.
    pub fn top_apps(&mut self, count: usize, by_memory: bool) -> Vec<AppUsage> {
        self.refresh_processes();
        let cores = self.system.cpus().len().max(1) as f32;
        let mut apps: Vec<AppUsage> = Vec::new();
        for process in self.system.processes().values() {
            let name = program_name(&process.name().to_string_lossy());
            // The idle "process" is the processor doing nothing.
            if name.is_empty() || name.eq_ignore_ascii_case("System Idle Process") || name.eq_ignore_ascii_case("Idle") {
                continue;
            }
            let cpu = process.cpu_usage() / 100.0 / cores;
            match apps.iter_mut().find(|a| a.name.eq_ignore_ascii_case(&name)) {
                Some(app) => {
                    app.cpu += cpu;
                    app.memory += process.memory();
                    app.processes += 1;
                }
                None => apps.push(AppUsage {
                    name,
                    cpu,
                    memory: process.memory(),
                    processes: 1,
                }),
            }
        }
        if by_memory {
            apps.sort_by_key(|a| std::cmp::Reverse(a.memory));
        } else {
            apps.sort_by(|a, b| b.cpu.total_cmp(&a.cpu));
        }
        apps.truncate(count);
        apps
    }

    /// Ends every process of the app. Returns how many were ended.
    pub fn end_app(&mut self, name: &str) -> usize {
        self.refresh_processes();
        self.system
            .processes()
            .values()
            .filter(|p| program_name(&p.name().to_string_lossy()).eq_ignore_ascii_case(name))
            .filter(|p| p.kill())
            .count()
    }

    /// Lowercase program names of everything running, without ".exe".
    pub fn running_programs(&mut self) -> HashSet<String> {
        self.refresh_processes();
        self.system
            .processes()
            .values()
            .map(|p| program_name(&p.name().to_string_lossy()).to_lowercase())
            .collect()
    }
}

/// "chrome.exe" -> "chrome".
pub fn program_name(name: &str) -> String {
    let name = name.trim();
    match name.len().checked_sub(4) {
        Some(cut) if name.is_char_boundary(cut) && name[cut..].eq_ignore_ascii_case(".exe") => name[..cut].to_string(),
        _ => name.to_string(),
    }
}

/// Battery charge from 0 to 100 and whether it's charging, or None without a battery.
#[cfg(windows)]
pub fn battery() -> Option<(u8, bool)> {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut status = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut status) }.ok()?;
    // 128: no battery; 255: unknown. 255 percent also means unknown.
    if status.BatteryFlag & 128 != 0 || status.BatteryFlag == 255 || status.BatteryLifePercent > 100 {
        return None;
    }
    Some((status.BatteryLifePercent, status.ACLineStatus == 1))
}

#[cfg(not(windows))]
pub fn battery() -> Option<(u8, bool)> {
    None
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_names() {
        assert_eq!(drive_name("d"), "D:");
        assert_eq!(drive_name("d:"), "D:");
        assert_eq!(drive_name(r"D:\"), "D:");
        assert_eq!(drive_name(r"C:\Mounts\Games\"), r"C:\Mounts\Games");
    }

    #[test]
    fn program_names() {
        assert_eq!(program_name("chrome.exe"), "chrome");
        assert_eq!(program_name("Discord.EXE"), "Discord");
        assert_eq!(program_name("System"), "System");
    }

    #[test]
    fn this_test_shows_up_in_the_process_list() {
        let mut stats = Stats::new();
        let me = std::env::current_exe().unwrap();
        let me = program_name(&me.file_name().unwrap().to_string_lossy()).to_lowercase();
        assert!(stats.running_programs().contains(&me));
        assert!(!stats.top_apps(5, true).is_empty());
    }

    #[test]
    fn system_drive_is_reported() {
        let mut stats = Stats::new();
        let sample = stats.sample(&[]);
        assert_eq!(sample.drives.len(), 1);
        assert!(sample.drives[0].total > 0);
    }
}
