//! Readings from HWiNFO: processor temperature, power, fan and clock, graphics card
//! readings for AMD and Intel cards (and the hot spot and memory temperatures NVIDIA's
//! library doesn't give), and any other sensor for a card of its own.
//!
//! Windows has no standard API for these, so the hub reads them from HWiNFO's shared
//! memory when HWiNFO is running with "Shared Memory Support" turned on in its settings.
//! Without it, these readings show as missing.

#[derive(Clone, Copy, Debug, Default)]
pub struct CpuSensors {
    pub temp_c: Option<f32>,
    pub power_w: Option<f32>,
    pub fan_rpm: Option<f32>,
    pub clock_mhz: Option<f32>,
}

/// The first graphics card's readings, as HWiNFO names them.
#[derive(Clone, Debug, Default)]
pub struct GpuSensors {
    /// Short name, e.g. "Radeon RX 7800 XT".
    pub name: Option<String>,
    /// 0.0 to 1.0
    pub load: Option<f32>,
    pub temp_c: Option<f32>,
    pub hotspot_c: Option<f32>,
    pub memory_c: Option<f32>,
    pub clock_mhz: Option<f32>,
    pub power_w: Option<f32>,
    /// Fan speed and whether it is in RPM (true) or percent (false).
    pub fan: Option<(f32, bool)>,
    /// Video memory used and in total, in bytes.
    pub vram: Option<(u64, u64)>,
}

// Reading types in HWiNFO's layout.
const TEMPERATURE: u32 = 1;
const FAN: u32 = 3;
const POWER: u32 = 5;
const CLOCK: u32 = 6;
const USAGE: u32 = 7;
const OTHER: u32 = 8;

/// One reading from HWiNFO.
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    /// The sensor (chip or device) it belongs to, e.g. "CPU [#0]: AMD Ryzen 7 7800X3D: Enhanced".
    pub sensor: String,
    /// The name as renamed in HWiNFO, if it was, else the original.
    pub label: String,
    pub original: String,
    /// "°C", "RPM", "W", "%"...
    pub unit: String,
    pub value: f64,
    pub kind: u32,
}

impl Reading {
    fn is(&self, names: &[&str]) -> Option<usize> {
        let label = self.label.to_lowercase();
        let original = self.original.to_lowercase();
        names.iter().position(|name| *name == label || *name == original)
    }

    /// The value in °C for temperatures HWiNFO shows in °F.
    fn celsius(&self) -> f32 {
        if self.unit.contains('F') {
            ((self.value - 32.0) * 5.0 / 9.0) as f32
        } else {
            self.value as f32
        }
    }
}

/// Everything HWiNFO is reporting right now.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub readings: Vec<Reading>,
}

#[cfg(windows)]
pub fn read() -> Option<Snapshot> {
    use windows::core::w;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Memory::{MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_READ};

    unsafe {
        let mapping = OpenFileMappingW(FILE_MAP_READ.0, false, w!("Global\\HWiNFO_SENS_SM2")).ok()?;
        let view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0);
        let result = if view.Value.is_null() { None } else { parse(view.Value as *const u8) };
        if !view.Value.is_null() {
            let _ = UnmapViewOfFile(view);
        }
        let _ = CloseHandle(mapping);
        result
    }
}

#[cfg(not(windows))]
pub fn read() -> Option<Snapshot> {
    None
}

/// HWiNFO's text is in the ANSI code page; for the characters it uses (letters, digits,
/// "°") that is Latin-1, one byte per character.
fn latin1(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    bytes[..end].iter().map(|&b| b as char).collect::<String>().trim().to_string()
}

/// Walks the shared memory block. Layout (packed, little-endian):
/// header: signature "HWiS", version, revision, poll time (8 bytes), then offset, size and
/// count of the sensor section and of the reading section. Each sensor: id, instance,
/// original name (128), user name (128). Each reading: type, sensor index, id, original
/// label (128), user label (128), unit (16), then value/min/max/avg as f64.
#[cfg(windows)]
unsafe fn parse(base: *const u8) -> Option<Snapshot> {
    let u32_at = |offset: usize| unsafe { (base.add(offset) as *const u32).read_unaligned() };
    if u32_at(0) != u32::from_le_bytes(*b"HWiS") {
        return None;
    }
    let (sensors_offset, sensor_size, sensor_count) = (u32_at(20) as usize, u32_at(24) as usize, u32_at(28) as usize);
    let (readings_offset, reading_size, reading_count) = (u32_at(32) as usize, u32_at(36) as usize, u32_at(40) as usize);
    if reading_size < 316 || reading_count > 10_000 || sensor_size < 264 || sensor_count > 1_000 {
        return None;
    }
    let text = |at: *const u8, len: usize| latin1(unsafe { std::slice::from_raw_parts(at, len) });

    let sensors: Vec<String> = (0..sensor_count)
        .map(|i| {
            let entry = unsafe { base.add(sensors_offset + i * sensor_size) };
            let user = text(unsafe { entry.add(136) }, 128);
            if user.is_empty() { text(unsafe { entry.add(8) }, 128) } else { user }
        })
        .collect();
    let readings = (0..reading_count)
        .map(|i| {
            let entry = unsafe { base.add(readings_offset + i * reading_size) };
            let kind = unsafe { (entry as *const u32).read_unaligned() };
            let sensor = unsafe { (entry.add(4) as *const u32).read_unaligned() } as usize;
            let original = text(unsafe { entry.add(12) }, 128);
            let label = text(unsafe { entry.add(140) }, 128);
            Reading {
                sensor: sensors.get(sensor).cloned().unwrap_or_default(),
                label: if label.is_empty() { original.clone() } else { label },
                original,
                unit: text(unsafe { entry.add(268) }, 16),
                value: unsafe { (entry.add(284) as *const f64).read_unaligned() },
                kind,
            }
        })
        .collect();
    Some(Snapshot { readings })
}

/// The best of several readings: the one whose name comes first in a list of preferred
/// names.
fn best<'a>(readings: impl Iterator<Item = &'a Reading>, kind: u32, names: &[&str]) -> Option<&'a Reading> {
    readings
        .filter(|r| r.kind == kind)
        .filter_map(|r| r.is(names).map(|rank| (rank, r)))
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, r)| r)
}

impl Snapshot {
    /// `cpu_fan` is the HWiNFO name of the processor fan (e.g. "System 1"), for boards
    /// that don't label it; otherwise the first fan with "CPU" in its name is used.
    pub fn cpu(&self, cpu_fan: Option<&str>) -> CpuSensors {
        let all = || self.readings.iter();
        let temp = best(
            all(),
            TEMPERATURE,
            &["cpu (tctl/tdie)", "cpu package", "cpu (tdie)", "cpu die (average)", "core temperatures (avg)"],
        );
        let power = best(all(), POWER, &["cpu package power", "cpu ppt", "cpu power"]);
        let cpu_fan = cpu_fan.map(str::to_lowercase).filter(|f| !f.is_empty());
        let fan = match &cpu_fan {
            Some(name) => all().find(|r| r.kind == FAN && (r.label.to_lowercase() == *name || r.original.to_lowercase() == *name)),
            None => {
                let fans: Vec<&Reading> = all().filter(|r| r.kind == FAN && r.label.to_lowercase().contains("cpu")).collect();
                fans.iter().find(|r| r.value > 0.0).or(fans.first()).copied()
            }
        };
        // Per-core frequencies ("Core 0 Clock"), averaged when there's no summary reading.
        // "Effective" clocks count sleep time as zero, so they read far below the real speed.
        let cores: Vec<f64> = all()
            .filter(|r| r.kind == CLOCK)
            .filter(|r| {
                let label = r.label.to_lowercase();
                label.starts_with("core ") && label.contains(" clock") && !label.contains("effective")
            })
            .map(|r| r.value)
            .collect();
        let clock = best(all(), CLOCK, &["core clocks (avg)"])
            .map(|r| r.value as f32)
            .or((!cores.is_empty()).then(|| (cores.iter().sum::<f64>() / cores.len() as f64) as f32));
        CpuSensors {
            temp_c: temp.map(Reading::celsius),
            power_w: power.map(|r| r.value as f32),
            fan_rpm: fan.map(|r| r.value as f32),
            clock_mhz: clock,
        }
    }

    /// The first graphics card HWiNFO lists ("GPU [#0]: ...").
    pub fn gpu(&self) -> GpuSensors {
        let Some(sensor) = self
            .readings
            .iter()
            .map(|r| &r.sensor)
            .find(|s| s.to_lowercase().starts_with("gpu [#"))
        else {
            return GpuSensors::default();
        };
        let all = || self.readings.iter().filter(|r| r.sensor == *sensor);
        let value = |r: &Reading| r.value as f32;
        let load = best(all(), USAGE, &["gpu core load", "gpu utilization", "gpu d3d usage", "gpu load"]);
        let fan_rpm = all().find(|r| r.kind == FAN && r.label.to_lowercase().contains("gpu fan"));
        let fan_pct = all().find(|r| r.kind == USAGE && r.label.to_lowercase().starts_with("gpu fan"));
        let used_mb = best(all(), OTHER, &["gpu memory allocated", "gpu d3d memory dedicated", "gpu memory used"]);
        let usage = best(all(), USAGE, &["gpu memory usage"]);
        const MB: f64 = 1024.0 * 1024.0;
        let vram = used_mb.zip(usage).filter(|(_, pct)| pct.value > 0.5).map(|(used, pct)| {
            let total_gb = (used.value / (pct.value / 100.0) / 1024.0).round().max(1.0);
            ((used.value * MB) as u64, (total_gb * 1024.0 * MB) as u64)
        });
        GpuSensors {
            name: gpu_name(sensor),
            load: load.map(|r| value(r) / 100.0),
            temp_c: best(all(), TEMPERATURE, &["gpu temperature", "gpu core temperature", "gpu thermal diode"]).map(Reading::celsius),
            hotspot_c: best(all(), TEMPERATURE, &["gpu hot spot temperature", "gpu hotspot temperature", "gpu hot spot"]).map(Reading::celsius),
            memory_c: best(
                all(),
                TEMPERATURE,
                &["gpu memory junction temperature", "gpu memory temperature", "gpu vram temperature"],
            )
            .map(Reading::celsius),
            clock_mhz: best(all(), CLOCK, &["gpu clock", "gpu core clock", "gpu shader clock"]).map(value),
            power_w: best(
                all(),
                POWER,
                &["gpu power", "total board power (tbp)", "total graphics power (tgp)", "gpu asic power", "gpu package power", "gpu core power"],
            )
            .map(value),
            fan: fan_rpm.map(|r| (value(r), true)).or(fan_pct.map(|r| (value(r), false))),
            vram,
        }
    }

    /// A reading picked for a card of its own: by its label (as renamed in HWiNFO, or the
    /// original), optionally only within sensors whose name contains `sensor`.
    pub fn find(&self, sensor: Option<&str>, reading: &str) -> Option<&Reading> {
        let reading = reading.trim().to_lowercase();
        let sensor = sensor.map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty());
        self.readings.iter().find(|r| {
            (r.label.to_lowercase() == reading || r.original.to_lowercase() == reading)
                && sensor.as_ref().is_none_or(|s| r.sensor.to_lowercase().contains(s))
        })
    }
}

/// "GPU [#0]: AMD Radeon RX 7800 XT: " -> "Radeon RX 7800 XT"
fn gpu_name(sensor: &str) -> Option<String> {
    let name = sensor.split_once(':')?.1.split(':').next()?.trim();
    let name = ["NVIDIA ", "GeForce ", "AMD ", "Intel(R) ", "Intel "]
        .iter()
        .fold(name.to_string(), |name, maker| name.replace(maker, ""));
    Some(name.trim().to_string()).filter(|n| !n.is_empty())
}

/// A reading as display text in the chosen temperature unit, and its unit.
pub fn display(reading: &Reading, fahrenheit: bool) -> (String, String) {
    if reading.kind == TEMPERATURE {
        let c = reading.celsius();
        return if fahrenheit {
            (format!("{:.0}", c * 9.0 / 5.0 + 32.0), "°F".into())
        } else {
            (format!("{c:.0}"), "°C".into())
        };
    }
    let value = reading.value;
    let text = if value.abs() >= 100.0 || reading.kind == FAN {
        format!("{value:.0}")
    } else if value.abs() >= 10.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.2}")
    };
    (text, reading.unit.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(sensor: &str, label: &str, unit: &str, value: f64, kind: u32) -> Reading {
        Reading {
            sensor: sensor.into(),
            label: label.into(),
            original: label.into(),
            unit: unit.into(),
            value,
            kind,
        }
    }

    #[test]
    fn graphics_cards_from_hwinfo() {
        let gpu = "GPU [#0]: AMD Radeon RX 7800 XT: ";
        let snapshot = Snapshot {
            readings: vec![
                reading("CPU [#0]: AMD Ryzen 7 7800X3D", "CPU (Tctl/Tdie)", "°C", 61.0, TEMPERATURE),
                reading(gpu, "GPU Temperature", "°C", 55.0, TEMPERATURE),
                reading(gpu, "GPU Hot Spot Temperature", "°C", 71.0, TEMPERATURE),
                reading(gpu, "GPU Memory Junction Temperature", "°C", 68.0, TEMPERATURE),
                reading(gpu, "GPU Core Load", "%", 40.0, USAGE),
                reading(gpu, "GPU Fan", "RPM", 1200.0, FAN),
                reading(gpu, "Total Board Power (TBP)", "W", 210.0, POWER),
                reading(gpu, "GPU Memory Allocated", "MB", 4096.0, OTHER),
                reading(gpu, "GPU Memory Usage", "%", 25.0, USAGE),
                reading("GPU [#1]: Intel UHD", "GPU Temperature", "°C", 40.0, TEMPERATURE),
            ],
        };
        let g = snapshot.gpu();
        assert_eq!(g.name.as_deref(), Some("Radeon RX 7800 XT"));
        assert_eq!((g.temp_c, g.hotspot_c, g.memory_c), (Some(55.0), Some(71.0), Some(68.0)));
        assert_eq!(g.load, Some(0.4));
        assert_eq!(g.fan, Some((1200.0, true)));
        assert_eq!(g.power_w, Some(210.0));
        assert_eq!(g.vram.map(|(_, total)| total / (1024 * 1024 * 1024)), Some(16));
        assert_eq!(snapshot.cpu(None).temp_c, Some(61.0));
        assert!(snapshot.find(Some("ryzen"), "cpu (tctl/tdie)").is_some());
        assert!(snapshot.find(Some("intel"), "cpu (tctl/tdie)").is_none());
    }

    #[test]
    fn display_text() {
        let water = reading("Aquacomputer", "Water", "°C", 31.4, TEMPERATURE);
        assert_eq!(display(&water, false), ("31".into(), "°C".into()));
        assert_eq!(display(&water, true), ("89".into(), "°F".into()));
        let flow = reading("Aquacomputer", "Flow", "l/h", 72.31, OTHER);
        assert_eq!(display(&flow, false), ("72.3".into(), "l/h".into()));
        assert_eq!(latin1(b"\xb0C\0junk"), "°C");
    }
}
