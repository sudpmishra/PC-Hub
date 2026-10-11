//! Windows settings that tiles switch on and off: Wi-Fi, Bluetooth, night light and Do Not
//! Disturb. Their states are read on a background thread every couple of seconds so tiles
//! can light up while they're on.
//!
//! Wi-Fi and Bluetooth use Windows' radio API. Night light and Do Not Disturb have no
//! public API: night light is a setting stored in the registry (as Settings itself stores
//! it), and Do Not Disturb is a system notification state (as the quick settings panel
//! changes it). Both have worked the same way since Windows 10, but a Windows update
//! could change them.

use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Which toggles are on; None when it couldn't be read (no Wi-Fi adapter, say).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct States {
    pub wifi: Option<bool>,
    pub bluetooth: Option<bool>,
    pub night_light: Option<bool>,
    pub dnd: Option<bool>,
}

/// Reads the states every two seconds into the returned value.
pub fn watch() -> Arc<Mutex<States>> {
    let shared = Arc::new(Mutex::new(States::default()));
    let writer = shared.clone();
    std::thread::Builder::new()
        .name("toggles".into())
        .spawn(move || loop {
            let states = read();
            if let Ok(mut shared) = writer.lock() {
                *shared = states;
            }
            std::thread::sleep(Duration::from_secs(2));
        })
        .expect("failed to start the toggles thread");
    shared
}

pub fn read() -> States {
    States {
        wifi: imp::radio(imp::Kind::WiFi),
        bluetooth: imp::radio(imp::Kind::Bluetooth),
        night_light: imp::night_light(),
        dnd: imp::dnd(),
    }
}

/// Switches "wifi", "bluetooth", "night-light" or "dnd". Blocks briefly.
pub fn toggle(name: &str) -> Result<(), String> {
    match name {
        "wifi" => imp::set_radio(imp::Kind::WiFi),
        "bluetooth" => imp::set_radio(imp::Kind::Bluetooth),
        "night-light" => imp::toggle_night_light(),
        "dnd" => imp::toggle_dnd(),
        other => Err(format!("unknown toggle \"{other}\"")),
    }
}

/// Night light's stored state, as Settings writes it: a header, then byte 18 is 0x15 when
/// on and 0x13 when off; "on" also has two more bytes (0x10, 0x00) after byte 22. Bytes
/// 10 to 14 are a timestamp that must change for Windows to notice.
fn night_light_on(data: &[u8]) -> Option<bool> {
    match data.get(18) {
        Some(0x15) => Some(true),
        Some(0x13) => Some(false),
        _ => None,
    }
}

fn flip_night_light(data: &[u8]) -> Option<Vec<u8>> {
    let on = night_light_on(data)?;
    if data.len() < 25 {
        return None;
    }
    let mut new: Vec<u8> = if on {
        // Drop the two "on" bytes.
        data[..23].iter().chain(&data[25..]).copied().collect()
    } else {
        data[..23].iter().chain(&[0x10, 0x00]).chain(&data[23..]).copied().collect()
    };
    new[18] = if on { 0x13 } else { 0x15 };
    for byte in &mut new[10..15] {
        if *byte != 0xff {
            *byte += 1;
            break;
        }
    }
    Some(new)
}

#[cfg(windows)]
mod imp {
    use windows::core::{w, PCWSTR};
    use windows::Devices::Radios::{Radio, RadioAccessStatus, RadioKind, RadioState};
    use windows::Win32::System::Registry::{RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_BINARY, RRF_RT_REG_BINARY};

    pub enum Kind {
        WiFi,
        Bluetooth,
    }

    fn find(kind: Kind) -> windows::core::Result<Option<Radio>> {
        let wanted = match kind {
            Kind::WiFi => RadioKind::WiFi,
            Kind::Bluetooth => RadioKind::Bluetooth,
        };
        let radios = Radio::GetRadiosAsync()?.join()?;
        Ok(radios.into_iter().find(|r| r.Kind().is_ok_and(|k| k == wanted)))
    }

    pub fn radio(kind: Kind) -> Option<bool> {
        let radio = find(kind).ok()??;
        Some(radio.State().ok()? == RadioState::On)
    }

    pub fn set_radio(kind: Kind) -> Result<(), String> {
        let name = match kind {
            Kind::WiFi => "Wi-Fi",
            Kind::Bluetooth => "Bluetooth",
        };
        let access = Radio::RequestAccessAsync().and_then(|op| op.join()).map_err(|e| e.to_string())?;
        if access != RadioAccessStatus::Allowed {
            return Err(format!("Windows didn't allow changing {name} (Settings > Privacy > Radios)"));
        }
        let radio = find(kind).map_err(|e| e.to_string())?.ok_or_else(|| format!("this PC has no {name}"))?;
        let on = radio.State().map_err(|e| e.to_string())? == RadioState::On;
        let result = radio
            .SetStateAsync(if on { RadioState::Off } else { RadioState::On })
            .and_then(|op| op.join())
            .map_err(|e| e.to_string())?;
        if result == RadioAccessStatus::Allowed {
            Ok(())
        } else {
            Err(format!("Windows didn't allow changing {name}"))
        }
    }

    const NIGHT_LIGHT_KEY: PCWSTR = w!(
        r"Software\Microsoft\Windows\CurrentVersion\CloudStore\Store\DefaultAccount\Current\default$windows.data.bluelightreduction.bluelightreductionstate\windows.data.bluelightreduction.bluelightreductionstate"
    );

    fn night_light_data() -> Option<Vec<u8>> {
        let mut size = 0u32;
        unsafe {
            RegGetValueW(HKEY_CURRENT_USER, NIGHT_LIGHT_KEY, w!("Data"), RRF_RT_REG_BINARY, None, None, Some(&mut size))
                .ok()
                .ok()?;
            let mut data = vec![0u8; size as usize];
            RegGetValueW(
                HKEY_CURRENT_USER,
                NIGHT_LIGHT_KEY,
                w!("Data"),
                RRF_RT_REG_BINARY,
                None,
                Some(data.as_mut_ptr().cast()),
                Some(&mut size),
            )
            .ok()
            .ok()?;
            data.truncate(size as usize);
            Some(data)
        }
    }

    pub fn night_light() -> Option<bool> {
        super::night_light_on(&night_light_data()?)
    }

    pub fn toggle_night_light() -> Result<(), String> {
        let data = night_light_data().ok_or("night light has never been set up on this PC (Settings > Display > Night light)")?;
        let new = super::flip_night_light(&data).ok_or("night light's setting is in a form the hub doesn't know")?;
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                NIGHT_LIGHT_KEY,
                w!("Data"),
                REG_BINARY.0,
                Some(new.as_ptr().cast()),
                new.len() as u32,
            )
        }
        .ok()
        .map_err(|e| e.to_string())
    }

    /// The notification state behind Focus / Do Not Disturb: 0 off, 1 priority only,
    /// 2 alarms only.
    const QUIET_HOURS: u64 = 0x0D83_063E_A3BF_1C75;

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtQueryWnfStateData(
            state: *const u64,
            type_id: *const std::ffi::c_void,
            scope: *const std::ffi::c_void,
            change_stamp: *mut u32,
            buffer: *mut std::ffi::c_void,
            size: *mut u32,
        ) -> i32;
        fn NtUpdateWnfStateData(
            state: *const u64,
            buffer: *const std::ffi::c_void,
            size: u32,
            type_id: *const std::ffi::c_void,
            scope: *const std::ffi::c_void,
            matching_stamp: u32,
            check_stamp: u32,
        ) -> i32;
    }

    fn quiet_hours() -> Option<u32> {
        let (mut stamp, mut value, mut size) = (0u32, 0u32, 4u32);
        let status = unsafe {
            NtQueryWnfStateData(
                &QUIET_HOURS,
                std::ptr::null(),
                std::ptr::null(),
                &mut stamp,
                (&mut value as *mut u32).cast(),
                &mut size,
            )
        };
        (status >= 0).then_some(if size == 0 { 0 } else { value })
    }

    pub fn dnd() -> Option<bool> {
        quiet_hours().map(|v| v != 0)
    }

    pub fn toggle_dnd() -> Result<(), String> {
        let value: u32 = if quiet_hours().unwrap_or(0) != 0 { 0 } else { 1 };
        let status = unsafe {
            NtUpdateWnfStateData(&QUIET_HOURS, (&value as *const u32).cast(), 4, std::ptr::null(), std::ptr::null(), 0, 0)
        };
        if status >= 0 {
            Ok(())
        } else {
            Err(format!("Windows refused to change Do Not Disturb (status {status:#x})"))
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub enum Kind {
        WiFi,
        Bluetooth,
    }
    pub fn radio(_: Kind) -> Option<bool> {
        None
    }
    pub fn set_radio(_: Kind) -> Result<(), String> {
        Err("only supported on Windows".into())
    }
    pub fn night_light() -> Option<bool> {
        None
    }
    pub fn toggle_night_light() -> Result<(), String> {
        Err("only supported on Windows".into())
    }
    pub fn dnd() -> Option<bool> {
        None
    }
    pub fn toggle_dnd() -> Result<(), String> {
        Err("only supported on Windows".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn night_light_round_trip() {
        let mut off = vec![0u8; 40];
        off[18] = 0x13;
        off[10] = 0x05;
        assert_eq!(night_light_on(&off), Some(false));
        let on = flip_night_light(&off).unwrap();
        assert_eq!(night_light_on(&on), Some(true));
        assert_eq!(on.len(), 42);
        assert_eq!((on[23], on[24]), (0x10, 0x00));
        assert_eq!(on[10], 0x06);
        let back = flip_night_light(&on).unwrap();
        assert_eq!(night_light_on(&back), Some(false));
        assert_eq!(back.len(), 40);
        assert_eq!(flip_night_light(&[0u8; 10]), None);
    }

    #[test]
    #[ignore = "reads this PC's settings"]
    fn reads_states() {
        println!("{:?}", read());
    }
}
