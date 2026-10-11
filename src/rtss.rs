//! Frames per second of the game in front, from RivaTuner Statistics Server (it comes with
//! MSI Afterburner). RTSS counts the frames of every 3D app it hooks and shares the counts
//! in memory; without it running, there's no reading.

/// The game in front: its frame rate and program name.
#[derive(Clone, Debug, PartialEq)]
pub struct Fps {
    pub fps: f32,
    pub program: String,
}

#[cfg(windows)]
pub fn read() -> Option<Fps> {
    use windows::core::w;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Memory::{MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_READ};
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    unsafe {
        let mut front = 0u32;
        GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut front));
        let mapping = OpenFileMappingW(FILE_MAP_READ.0, false, w!("RTSSSharedMemoryV2")).ok()?;
        let view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0);
        let result = if view.Value.is_null() { None } else { parse(view.Value as *const u8, front) };
        if !view.Value.is_null() {
            let _ = UnmapViewOfFile(view);
        }
        let _ = CloseHandle(mapping);
        result
    }
}

#[cfg(not(windows))]
pub fn read() -> Option<Fps> {
    None
}

/// The layout (RTSS_SHARED_MEMORY, version 2): signature "RTSS", version, app entry size,
/// app array offset and size, ... Each app entry: process id, name (260 bytes), flags,
/// time0 and time1 (ms), frames counted between them, ...
#[cfg(windows)]
unsafe fn parse(base: *const u8, front: u32) -> Option<Fps> {
    let u32_at = |at: *const u8| unsafe { (at as *const u32).read_unaligned() };
    if u32_at(base) != u32::from_be_bytes(*b"RTSS") || u32_at(unsafe { base.add(4) }) < 0x0002_0000 {
        return None;
    }
    let entry_size = u32_at(unsafe { base.add(8) }) as usize;
    let offset = u32_at(unsafe { base.add(12) }) as usize;
    let count = u32_at(unsafe { base.add(16) }) as usize;
    if entry_size < 284 || count > 1024 {
        return None;
    }
    (0..count).find_map(|i| {
        let entry = unsafe { base.add(offset + i * entry_size) };
        let pid = u32_at(entry);
        if pid == 0 || pid != front {
            return None;
        }
        let name = unsafe { std::slice::from_raw_parts(entry.add(4), 260) };
        let (time0, time1, frames) = (u32_at(unsafe { entry.add(268) }), u32_at(unsafe { entry.add(272) }), u32_at(unsafe { entry.add(276) }));
        fps(time0, time1, frames).map(|fps| Fps { fps, program: program(name) })
    })
}

/// Frames counted over a period of milliseconds, as frames per second.
fn fps(time0: u32, time1: u32, frames: u32) -> Option<f32> {
    let elapsed = time1.wrapping_sub(time0);
    (elapsed > 0 && elapsed < 10_000 && frames > 0).then(|| frames as f32 * 1000.0 / elapsed as f32)
}

/// "C:\Games\cs2.exe" -> "cs2"
fn program(name: &[u8]) -> String {
    let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
    let path = String::from_utf8_lossy(&name[..end]).into_owned();
    crate::stats::program_name(path.rsplit(['\\', '/']).next().unwrap_or(&path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_rates() {
        assert_eq!(fps(1000, 2000, 144), Some(144.0));
        assert_eq!(fps(1000, 1500, 30), Some(60.0));
        assert_eq!(fps(5, 5, 10), None);
        assert_eq!(program(b"C:\\Games\\cs2.exe\0\0"), "cs2");
    }
}
