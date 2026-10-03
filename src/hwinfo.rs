//! Processor temperature, power, fan and effective clock from HWiNFO.
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

/// `cpu_fan` is the HWiNFO name of the processor fan (e.g. "System 1"), for boards
/// that don't label it; otherwise the first fan with "CPU" in its name is used.
#[cfg(windows)]
pub fn read(cpu_fan: Option<&str>) -> Option<CpuSensors> {
    use windows::core::w;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Memory::{
        MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_READ,
    };

    unsafe {
        let mapping = OpenFileMappingW(FILE_MAP_READ.0, false, w!("Global\\HWiNFO_SENS_SM2")).ok()?;
        let view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0);
        let result = if view.Value.is_null() {
            None
        } else {
            parse(view.Value as *const u8, &cpu_fan.unwrap_or("").to_lowercase())
        };
        if !view.Value.is_null() {
            let _ = UnmapViewOfFile(view);
        }
        let _ = CloseHandle(mapping);
        result
    }
}

#[cfg(not(windows))]
pub fn read(_cpu_fan: Option<&str>) -> Option<CpuSensors> {
    None
}

// Reading types in HWiNFO's layout.
const TEMPERATURE: u32 = 1;
const FAN: u32 = 3;
const POWER: u32 = 5;
const CLOCK: u32 = 6;

/// Walks the shared memory block. Layout (packed, little-endian):
/// header: signature "HWiS", version, revision, poll time (8 bytes), then offset, size and
/// count of the sensor section and of the reading section. Each reading: type, sensor index,
/// id, original label (128), user label (128), unit (16), then value/min/max/avg as f64.
#[cfg(windows)]
unsafe fn parse(base: *const u8, cpu_fan: &str) -> Option<CpuSensors> {
    let u32_at = |offset: usize| unsafe { (base.add(offset) as *const u32).read_unaligned() };
    if u32_at(0) != u32::from_le_bytes(*b"HWiS") {
        return None;
    }
    let readings_offset = u32_at(32) as usize;
    let reading_size = u32_at(36) as usize;
    let reading_count = u32_at(40) as usize;
    if reading_size < 316 || reading_count > 10_000 {
        return None;
    }

    let mut best: [(usize, Option<f32>); 4] = [(usize::MAX, None); 4];
    // Per-core frequencies ("Core 0 Clock"), averaged when there's no summary reading.
    // "Effective" clocks count sleep time as zero, so they read far below the real speed.
    let (mut core_clock_sum, mut core_clocks) = (0.0f32, 0u32);
    for i in 0..reading_count {
        let entry = unsafe { base.add(readings_offset + i * reading_size) };
        let kind = unsafe { (entry as *const u32).read_unaligned() };
        let text = |offset: usize, len: usize| {
            let bytes = unsafe { std::slice::from_raw_parts(entry.add(offset), len) };
            let end = bytes.iter().position(|&b| b == 0).unwrap_or(len);
            String::from_utf8_lossy(&bytes[..end]).to_lowercase()
        };
        // The name as renamed in HWiNFO, if it was, and the original.
        let original = text(12, 128);
        let label = text(140, 128);
        let label = if label.is_empty() { original.clone() } else { label };
        let unit = text(268, 16);
        let mut value = unsafe { (entry.add(284) as *const f64).read_unaligned() } as f32;

        let (slot, rank) = match kind {
            TEMPERATURE => {
                if unit.contains('f') {
                    value = (value - 32.0) * 5.0 / 9.0;
                }
                (0, rank(&label, &["cpu (tctl/tdie)", "cpu package", "cpu (tdie)", "cpu die (average)", "core temperatures (avg)"]))
            }
            POWER => (1, rank(&label, &["cpu package power", "cpu ppt", "cpu power"])),
            FAN => {
                let rank = if !cpu_fan.is_empty() {
                    (label == cpu_fan || original == cpu_fan).then_some(0)
                } else if label.contains("cpu") {
                    Some(if value > 0.0 { 0 } else { 1 })
                } else {
                    None
                };
                (2, rank)
            }
            CLOCK => {
                if label.starts_with("core ") && label.contains(" clock") && !label.contains("effective") {
                    core_clock_sum += value;
                    core_clocks += 1;
                }
                (3, rank(&label, &["core clocks (avg)"]))
            }
            _ => continue,
        };
        if let Some(rank) = rank {
            if rank < best[slot].0 {
                best[slot] = (rank, Some(value));
            }
        }
    }

    Some(CpuSensors {
        temp_c: best[0].1,
        power_w: best[1].1,
        fan_rpm: best[2].1,
        clock_mhz: best[3]
            .1
            .or((core_clocks > 0).then(|| core_clock_sum / core_clocks as f32)),
    })
}

/// Position of the first preferred name that matches the label exactly.
fn rank(label: &str, preferred: &[&str]) -> Option<usize> {
    preferred.iter().position(|name| *name == label)
}
