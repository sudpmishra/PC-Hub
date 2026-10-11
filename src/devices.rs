//! Battery levels of wireless headsets, controllers, mice and phones, as Windows shows
//! them in Settings > Bluetooth & devices. Devices on a maker's own dongle (many gaming
//! mice and headsets) don't tell Windows, so they only show in the maker's app.

/// A device and its charge, 0 to 100.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceBattery {
    pub name: String,
    pub percent: u8,
}

/// Each device once, sorted by name. Takes a moment, so call it from a background thread.
#[cfg(windows)]
pub fn batteries() -> Vec<DeviceBattery> {
    use windows::core::GUID;
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW, SetupDiGetDevicePropertyW,
        DIGCF_ALLCLASSES, DIGCF_PRESENT, SP_DEVINFO_DATA,
    };
    use windows::Win32::Devices::Properties::DEVPROPTYPE;
    use windows::Win32::Foundation::DEVPROPKEY;

    // The battery level Windows reads from Bluetooth devices.
    const BATTERY: DEVPROPKEY = DEVPROPKEY {
        fmtid: GUID::from_u128(0x104ea319_6ee2_4701_bd47_8ddbf425bbe5),
        pid: 2,
    };
    const FRIENDLY_NAME: DEVPROPKEY = DEVPROPKEY {
        fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
        pid: 14,
    };

    let mut found: Vec<DeviceBattery> = Vec::new();
    unsafe {
        let Ok(list) = SetupDiGetClassDevsW(None, None, None, DIGCF_ALLCLASSES | DIGCF_PRESENT) else {
            return found;
        };
        let mut index = 0;
        loop {
            let mut info = SP_DEVINFO_DATA {
                cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
                ..Default::default()
            };
            if SetupDiEnumDeviceInfo(list, index, &mut info).is_err() {
                break;
            }
            index += 1;
            let mut kind = DEVPROPTYPE::default();
            let mut level = [0u8; 4];
            let mut size = 0u32;
            if SetupDiGetDevicePropertyW(list, &info, &BATTERY, &mut kind, Some(&mut level), Some(&mut size), 0).is_err()
                || size == 0
            {
                continue;
            }
            let mut name = [0u16; 256];
            let mut name_size = 0u32;
            let name_bytes = std::slice::from_raw_parts_mut(name.as_mut_ptr() as *mut u8, name.len() * 2);
            if SetupDiGetDevicePropertyW(list, &info, &FRIENDLY_NAME, &mut kind, Some(name_bytes), Some(&mut name_size), 0).is_err() {
                continue;
            }
            let chars = (name_size as usize / 2).min(name.len());
            let name = clean_name(&String::from_utf16_lossy(&name[..chars]));
            if level[0] <= 100 && !name.is_empty() && !found.iter().any(|d| d.name == name) {
                found.push(DeviceBattery { name, percent: level[0] });
            }
        }
        let _ = SetupDiDestroyDeviceInfoList(list);
    }
    found.sort_by_key(|d| d.name.to_lowercase());
    found
}

#[cfg(not(windows))]
pub fn batteries() -> Vec<DeviceBattery> {
    Vec::new()
}

/// Bluetooth headsets show up once per profile: "WH-1000XM6 Hands-Free AG" and
/// "WH-1000XM6 Avrcp Transport" are the same headset.
fn clean_name(name: &str) -> String {
    let mut name = name.trim_end_matches('\0').trim().to_string();
    for suffix in [" Hands-Free AG", " Hands-Free HF", " Hands-Free", " Avrcp Transport", " Stereo", " LE"] {
        if let Some(stripped) = name.strip_suffix(suffix) {
            name = stripped.trim().to_string();
        }
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_suffixes_go() {
        assert_eq!(clean_name("WH-1000XM6 Hands-Free AG\0"), "WH-1000XM6");
        assert_eq!(clean_name("Pixel 8 Pro Hands-Free HF"), "Pixel 8 Pro");
        assert_eq!(clean_name("Xbox Wireless Controller"), "Xbox Wireless Controller");
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "depends on the devices paired with this PC"]
    fn lists_batteries() {
        println!("{:?}", batteries());
    }
}
