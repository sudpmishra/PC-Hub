//! Things a shortcut can do besides opening something: press a key combination
//! (for app hotkeys such as Discord mute or NVIDIA replay) or a built-in system action.

#[cfg(windows)]
mod imp {
    use std::time::Duration;

    use windows::Win32::System::Power::SetSuspendState;
    use windows::Win32::System::Shutdown::LockWorkStation;
    use windows::Win32::UI::Input::KeyboardAndMouse::*;

    /// Presses a combination such as "Ctrl+Shift+M": all keys down in order, then up in reverse.
    /// Runs on its own thread so the short hold doesn't stall the UI.
    pub fn send_keys(spec: &str) -> Result<(), String> {
        let keys = spec
            .split('+')
            .map(|name| key_code(name.trim()).ok_or_else(|| format!("unknown key \"{}\"", name.trim())))
            .collect::<Result<Vec<_>, _>>()?;
        if keys.is_empty() {
            return Err("no keys given".into());
        }
        std::thread::spawn(move || {
            let down: Vec<INPUT> = keys.iter().map(|&k| key_input(k, false)).collect();
            let up: Vec<INPUT> = keys.iter().rev().map(|&k| key_input(k, true)).collect();
            unsafe {
                SendInput(&down, std::mem::size_of::<INPUT>() as i32);
                // Some hotkey listeners miss presses that are released instantly.
                std::thread::sleep(Duration::from_millis(40));
                SendInput(&up, std::mem::size_of::<INPUT>() as i32);
            }
        });
        Ok(())
    }

    pub fn system(action: &str) -> Result<(), String> {
        match action {
            "lock" => unsafe { LockWorkStation() }.map_err(|e| e.to_string()),
            "sleep" => {
                if unsafe { SetSuspendState(false, false, false) } {
                    Ok(())
                } else {
                    Err("Windows refused to sleep".into())
                }
            }
            other => Err(format!("unknown action \"{other}\" (use lock or sleep)")),
        }
    }

    fn key_input(vk: VIRTUAL_KEY, up: bool) -> INPUT {
        let mut flags = if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) };
        if is_extended(vk) {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    fn is_extended(vk: VIRTUAL_KEY) -> bool {
        matches!(
            vk,
            VK_INSERT | VK_DELETE | VK_HOME | VK_END | VK_PRIOR | VK_NEXT | VK_LEFT | VK_RIGHT
                | VK_UP | VK_DOWN | VK_LWIN | VK_RCONTROL | VK_RMENU | VK_SNAPSHOT
        )
    }

    fn key_code(name: &str) -> Option<VIRTUAL_KEY> {
        let lower = name.to_ascii_lowercase();
        let vk = match lower.as_str() {
            "ctrl" | "control" => VK_CONTROL,
            "shift" => VK_SHIFT,
            "alt" => VK_MENU,
            "win" | "windows" | "super" => VK_LWIN,
            "esc" | "escape" => VK_ESCAPE,
            "enter" | "return" => VK_RETURN,
            "space" => VK_SPACE,
            "tab" => VK_TAB,
            "backspace" => VK_BACK,
            "delete" | "del" => VK_DELETE,
            "insert" | "ins" => VK_INSERT,
            "home" => VK_HOME,
            "end" => VK_END,
            "pageup" | "pgup" => VK_PRIOR,
            "pagedown" | "pgdn" => VK_NEXT,
            "left" => VK_LEFT,
            "right" => VK_RIGHT,
            "up" => VK_UP,
            "down" => VK_DOWN,
            "printscreen" | "prtsc" => VK_SNAPSHOT,
            "pause" => VK_PAUSE,
            "mute" | "volumemute" => VK_VOLUME_MUTE,
            "volumeup" => VK_VOLUME_UP,
            "volumedown" => VK_VOLUME_DOWN,
            "playpause" => VK_MEDIA_PLAY_PAUSE,
            "nexttrack" => VK_MEDIA_NEXT_TRACK,
            "prevtrack" => VK_MEDIA_PREV_TRACK,
            _ => {
                if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u16>().ok()) {
                    if (1..=24).contains(&n) {
                        return Some(VIRTUAL_KEY(VK_F1.0 + n - 1));
                    }
                }
                let mut chars = name.chars();
                return match (chars.next(), chars.next()) {
                    (Some(c), None) if c.is_ascii_alphanumeric() => {
                        Some(VIRTUAL_KEY(c.to_ascii_uppercase() as u16))
                    }
                    _ => None,
                };
            }
        };
        Some(vk)
    }

    /// Whether this Windows session is on the lock screen.
    pub fn session_locked() -> bool {
        use windows::Win32::System::RemoteDesktop::{
            WTSFreeMemory, WTSQuerySessionInformationW, WTSSessionInfoEx, WTSINFOEXW,
            WTS_CURRENT_SERVER_HANDLE, WTS_CURRENT_SESSION, WTS_SESSIONSTATE_LOCK,
        };

        unsafe {
            let mut buffer = windows::core::PWSTR::null();
            let mut bytes = 0u32;
            if WTSQuerySessionInformationW(
                Some(WTS_CURRENT_SERVER_HANDLE),
                WTS_CURRENT_SESSION,
                WTSSessionInfoEx,
                &mut buffer,
                &mut bytes,
            )
            .is_err()
                || buffer.is_null()
            {
                return false;
            }
            let info = &*(buffer.0 as *const WTSINFOEXW);
            let locked = info.Level == 1
                && info.Data.WTSInfoExLevel1.SessionFlags == WTS_SESSIONSTATE_LOCK as i32;
            WTSFreeMemory(buffer.0 as *mut _);
            locked
        }
    }

    /// Stops taps on the hub from activating its window, so the game or app in front
    /// keeps focus (and receives any keys a shortcut sends).
    pub fn make_non_activating(window: &slint::Window) {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{
            GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_NOACTIVATE,
        };

        let handle = window.window_handle();
        let Ok(handle) = handle.window_handle() else {
            return;
        };
        if let RawWindowHandle::Win32(win32) = handle.as_raw() {
            let hwnd = HWND(win32.hwnd.get() as *mut _);
            unsafe {
                let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | WS_EX_NOACTIVATE.0 as isize);
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn send_keys(_spec: &str) -> Result<(), String> {
        Err("sending keys is only supported on Windows".into())
    }

    pub fn system(_action: &str) -> Result<(), String> {
        Err("system actions are only supported on Windows".into())
    }

    pub fn make_non_activating(_window: &slint::Window) {}

    pub fn session_locked() -> bool {
        false
    }
}

pub use imp::*;

/// Opens a text file for editing.
pub fn edit_file(path: &std::path::Path) {
    let editor = if cfg!(windows) { "notepad" } else { "xdg-open" };
    if let Err(err) = std::process::Command::new(editor).arg(path).spawn() {
        eprintln!("could not open {}: {err}", path.display());
    }
}
