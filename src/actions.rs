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
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::System::Shutdown::{ExitWindowsEx, EWX_LOGOFF, SHUTDOWN_REASON};
        use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, HWND_BROADCAST, SC_MONITORPOWER, WM_SYSCOMMAND};

        let suspend = |hibernate: bool| {
            if unsafe { SetSuspendState(hibernate, false, false) } {
                Ok(())
            } else {
                Err(format!("Windows refused to {}", if hibernate { "hibernate" } else { "sleep" }))
            }
        };
        match action {
            "lock" => unsafe { LockWorkStation() }.map_err(|e| e.to_string()),
            "sleep" => suspend(false),
            "hibernate" => suspend(true),
            // shutdown.exe takes care of the shutdown privilege that ExitWindowsEx would need.
            "shutdown" => crate::config::spawn("shutdown", &["/s".into(), "/t".into(), "0".into()]).map_err(|e| e.to_string()),
            "restart" => crate::config::spawn("shutdown", &["/r".into(), "/t".into(), "0".into()]).map_err(|e| e.to_string()),
            "signout" => unsafe { ExitWindowsEx(EWX_LOGOFF, SHUTDOWN_REASON(0)) }.map_err(|e| e.to_string()),
            // 2 = off. Any touch, key press or mouse move turns the screens back on.
            "screen-off" => unsafe {
                PostMessageW(Some(HWND_BROADCAST), WM_SYSCOMMAND, WPARAM(SC_MONITORPOWER as usize), LPARAM(2))
            }
            .map_err(|e| e.to_string()),
            "empty-recycle-bin" => {
                use windows::Win32::UI::Shell::{SHEmptyRecycleBinW, SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI, SHERB_NOSOUND};
                // Runs on its own thread: a full bin can take a while.
                std::thread::spawn(|| unsafe {
                    let flags = SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND;
                    // Fails when the bin is already empty, which is fine.
                    let _ = SHEmptyRecycleBinW(None, windows::core::PCWSTR::null(), flags);
                });
                Ok(())
            }
            // The built-in plans' aliases: SCHEME_MAX saves the most power, SCHEME_MIN the least.
            "power-saver" => run_hidden("powercfg", &["/setactive", "SCHEME_MAX"]),
            "balanced" => run_hidden("powercfg", &["/setactive", "SCHEME_BALANCED"]),
            "high-performance" => run_hidden("powercfg", &["/setactive", "SCHEME_MIN"]),
            "dark-mode" => set_windows_light_theme(!windows_light_theme()),
            "wifi" | "bluetooth" | "night-light" | "dnd" => crate::toggles::toggle(action),
            other => Err(format!(
                "unknown action \"{other}\" (use lock, sleep, hibernate, shutdown, restart, signout, \
                 screen-off, mic-mute, mute, dark-mode, wifi, bluetooth, night-light, dnd, \
                 empty-recycle-bin, power-saver, balanced or high-performance)"
            )),
        }
    }

    /// Runs a command-line tool without flashing a console window.
    fn run_hidden(program: &str, args: &[&str]) -> Result<(), String> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        std::process::Command::new(program)
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map(drop)
            .map_err(|e| e.to_string())
    }

    /// Sends this process's error output (every eprintln!) to a file. Release builds have
    /// no console, so without this their messages are lost.
    pub fn log_to_file(file: std::fs::File) {
        use std::os::windows::io::IntoRawHandle;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Console::{SetStdHandle, STD_ERROR_HANDLE};
        // The handle stays open for the life of the process.
        let handle = HANDLE(file.into_raw_handle());
        if let Err(err) = unsafe { SetStdHandle(STD_ERROR_HANDLE, handle) } {
            eprintln!("log: could not write to the log file: {err}");
        }
    }

    /// "Ctrl+Alt+1" as RegisterHotKey's modifier flags and key code. Needs one key besides
    /// the modifiers.
    pub fn parse_hotkey(spec: &str) -> Result<(HOT_KEY_MODIFIERS, u32), String> {
        let mut modifiers = MOD_NOREPEAT;
        let mut key = None;
        for name in spec.split('+').map(str::trim) {
            match key_code(name).ok_or_else(|| format!("unknown key \"{name}\""))? {
                VK_CONTROL => modifiers |= MOD_CONTROL,
                VK_SHIFT => modifiers |= MOD_SHIFT,
                VK_MENU => modifiers |= MOD_ALT,
                VK_LWIN => modifiers |= MOD_WIN,
                vk if key.is_none() => key = Some(vk.0 as u32),
                _ => return Err(format!("\"{spec}\" has more than one key besides Ctrl, Shift, Alt and Win")),
            }
        }
        key.map(|k| (modifiers, k)).ok_or_else(|| format!("\"{spec}\" needs a key besides Ctrl, Shift, Alt and Win"))
    }

    const RUN_KEY: windows::core::PCWSTR = windows::core::w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
    const RUN_NAME: windows::core::PCWSTR = windows::core::w!("PC Hub");

    /// Plays Windows' notification sound, for the timer.
    pub fn chime() {
        use windows::Win32::Media::Audio::{PlaySoundW, SND_ALIAS, SND_ASYNC};
        let _ = unsafe { PlaySoundW(windows::core::w!("SystemNotification"), None, SND_ALIAS | SND_ASYNC) };
    }

    /// Whether Windows is set to light mode for apps (Settings > Personalization > Colors).
    pub fn windows_light_theme() -> bool {
        use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
        let mut value = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        let result = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                windows::core::w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
                windows::core::w!("AppsUseLightTheme"),
                RRF_RT_REG_DWORD,
                None,
                Some((&mut value as *mut u32).cast()),
                Some(&mut size),
            )
        };
        result.is_ok() && value != 0
    }

    /// Switches Windows' apps and taskbar between light and dark, as Settings >
    /// Personalization > Colors does, and tells open windows to repaint.
    fn set_windows_light_theme(light: bool) -> Result<(), String> {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_DWORD};
        use windows::Win32::UI::WindowsAndMessaging::{
            SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
        };
        let value = u32::from(light);
        for name in [windows::core::w!("AppsUseLightTheme"), windows::core::w!("SystemUsesLightTheme")] {
            unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    windows::core::w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
                    name,
                    REG_DWORD.0,
                    Some((&value as *const u32).cast()),
                    std::mem::size_of::<u32>() as u32,
                )
            }
            .ok()
            .map_err(|e| e.to_string())?;
        }
        let topic = windows::core::w!("ImmersiveColorSet");
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                WPARAM(0),
                LPARAM(topic.as_ptr() as isize),
                SMTO_ABORTIFHUNG,
                200,
                None,
            );
        }
        Ok(())
    }

    /// Whether Windows starts the hub at sign-in (an entry under the user's Run key).
    pub fn start_with_windows() -> bool {
        use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
        unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_NAME, RRF_RT_REG_SZ, None, None, None) }.is_ok()
    }

    pub fn set_start_with_windows(on: bool) -> Result<(), String> {
        use windows::Win32::System::Registry::{RegDeleteKeyValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
        let result = if on {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let command: Vec<u16> = format!("\"{}\"", exe.display()).encode_utf16().chain([0]).collect();
            unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    RUN_KEY,
                    RUN_NAME,
                    REG_SZ.0,
                    Some(command.as_ptr().cast()),
                    (command.len() * 2) as u32,
                )
            }
        } else {
            unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_NAME) }
        };
        result.ok().map_err(|e| e.to_string())
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

    pub(super) fn key_code(name: &str) -> Option<VIRTUAL_KEY> {
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

    fn hwnd_of(window: &slint::Window) -> Option<windows::Win32::Foundation::HWND> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let handle = window.window_handle();
        match handle.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(win32) => Some(windows::Win32::Foundation::HWND(win32.hwnd.get() as *mut _)),
            _ => None,
        }
    }

    /// The window's native handle, as a number.
    pub fn native_handle(window: &slint::Window) -> Option<isize> {
        hwnd_of(window).map(|hwnd| hwnd.0 as isize)
    }

    /// Presses `keys` in the main window of a running program (by its .exe name without
    /// ".exe"), then puts the app that was in front back. For app shortcuts that only work
    /// while the app has focus, such as Spotify's Like (Alt+Shift+B).
    pub fn press_in_app(program: &str, keys: &str) -> Result<(), String> {
        use windows::core::BOOL;
        use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
        use windows::Win32::System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            EnumWindows, GetForegroundWindow, GetWindowTextLengthW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
            SetForegroundWindow,
        };

        struct Search {
            program: String,
            found: Option<HWND>,
        }
        unsafe extern "system" fn visit(hwnd: HWND, data: LPARAM) -> BOOL {
            let search = unsafe { &mut *(data.0 as *mut Search) };
            unsafe {
                if !IsWindowVisible(hwnd).as_bool() || GetWindowTextLengthW(hwnd) == 0 {
                    return true.into();
                }
                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                    return true.into();
                };
                let mut buffer = [0u16; 1024];
                let mut size = buffer.len() as u32;
                let named = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, windows::core::PWSTR(buffer.as_mut_ptr()), &mut size);
                let _ = CloseHandle(process);
                if named.is_ok() {
                    let path = String::from_utf16_lossy(&buffer[..size as usize]);
                    let name = crate::stats::program_name(path.rsplit(['\\', '/']).next().unwrap_or(&path));
                    if name.eq_ignore_ascii_case(&search.program) {
                        search.found = Some(hwnd);
                        return false.into();
                    }
                }
            }
            true.into()
        }

        let mut search = Search { program: program.to_string(), found: None };
        unsafe {
            let _ = EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize));
        }
        let target = search.found.ok_or_else(|| format!("open {program}'s window first"))?;
        if unsafe { IsIconic(target) }.as_bool() {
            return Err(format!("{program} is minimized; restore its window first"));
        }
        let keys = keys.to_string();
        // Window handles are plain numbers, safe to pass to another thread.
        let target = target.0 as isize;
        std::thread::spawn(move || unsafe {
            let target = HWND(target as *mut _);
            let before = GetForegroundWindow();
            // Windows only lets an app bring another forward right after a key press.
            let alt = [key_input(VK_MENU, false), key_input(VK_MENU, true)];
            SendInput(&alt, std::mem::size_of::<INPUT>() as i32);
            let _ = SetForegroundWindow(target);
            std::thread::sleep(Duration::from_millis(120));
            if send_keys(&keys).is_ok() {
                std::thread::sleep(Duration::from_millis(250));
            }
            if !before.is_invalid() && before != target {
                let _ = SetForegroundWindow(before);
            }
        });
        Ok(())
    }

    /// Windows' display language as a short code, such as "de" or "en".
    pub fn windows_language() -> String {
        use windows::Win32::Globalization::GetUserDefaultLocaleName;
        let mut buffer = [0u16; 85];
        let length = unsafe { GetUserDefaultLocaleName(&mut buffer) };
        let name = String::from_utf16_lossy(&buffer[..(length.max(1) as usize - 1).min(buffer.len())]);
        name.split(['-', '_']).next().unwrap_or("en").to_lowercase()
    }

    fn set_non_activating(hwnd: windows::Win32::Foundation::HWND, on: bool) {
        use windows::Win32::UI::WindowsAndMessaging::{GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_NOACTIVATE};
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let flag = WS_EX_NOACTIVATE.0 as isize;
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, if on { style | flag } else { style & !flag });
        }
    }

    /// Stops taps on the hub from activating its window, so the game or app in front
    /// keeps focus (and receives any keys a shortcut sends).
    pub fn make_non_activating(window: &slint::Window) {
        if let Some(hwnd) = hwnd_of(window) {
            set_non_activating(hwnd, true);
        }
    }

    /// The window that had keyboard focus before the hub took it, to hand it back.
    static FOCUS_BEFORE: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

    /// While the on-screen keyboard is open, the hub takes keyboard focus so a physical
    /// keyboard can type into it too. Afterwards it gives focus back to the app that had it
    /// and goes back to never taking focus.
    pub fn take_keyboard_focus(window: &slint::Window, take: bool) {
        use std::sync::atomic::Ordering;
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, SetForegroundWindow};

        let Some(hwnd) = hwnd_of(window) else { return };
        unsafe {
            if take {
                let before = GetForegroundWindow();
                if before != hwnd {
                    FOCUS_BEFORE.store(before.0 as isize, Ordering::SeqCst);
                }
                set_non_activating(hwnd, false);
                // Allowed because the tap that opened the keyboard was input to the hub.
                if !SetForegroundWindow(hwnd).as_bool() {
                    eprintln!("keyboard: Windows didn't give the hub keyboard focus");
                }
            } else {
                set_non_activating(hwnd, true);
                let before = HWND(FOCUS_BEFORE.swap(0, Ordering::SeqCst) as *mut _);
                if !before.is_invalid() && GetForegroundWindow() == hwnd {
                    let _ = SetForegroundWindow(before);
                }
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

    pub fn take_keyboard_focus(_window: &slint::Window, _take: bool) {}

    pub fn native_handle(_window: &slint::Window) -> Option<isize> {
        None
    }

    pub fn windows_language() -> String {
        "en".into()
    }

    pub fn press_in_app(_program: &str, _keys: &str) -> Result<(), String> {
        Err("only supported on Windows".into())
    }

    pub fn session_locked() -> bool {
        false
    }

    pub fn start_with_windows() -> bool {
        false
    }

    pub fn log_to_file(_file: std::fs::File) {}

    pub fn windows_light_theme() -> bool {
        false
    }

    pub fn chime() {}

    pub fn set_start_with_windows(_on: bool) -> Result<(), String> {
        Err("starting with the system is only supported on Windows".into())
    }
}

pub use imp::*;

#[cfg(all(test, windows))]
mod tests {
    use super::imp::key_code;
    use windows::Win32::UI::Input::KeyboardAndMouse::*;

    #[test]
    fn key_names() {
        assert_eq!(key_code("Ctrl"), Some(VK_CONTROL));
        assert_eq!(key_code("win"), Some(VK_LWIN));
        assert_eq!(key_code("F10"), Some(VK_F10));
        assert_eq!(key_code("f24"), Some(VK_F24));
        assert_eq!(key_code("m"), Some(VIRTUAL_KEY(b'M' as u16)));
        assert_eq!(key_code("7"), Some(VIRTUAL_KEY(b'7' as u16)));
        assert_eq!(key_code("PageDown"), Some(VK_NEXT));
        assert_eq!(key_code("F25"), None);
        assert_eq!(key_code("Hyper"), None);
    }

    #[test]
    fn hotkeys() {
        let (modifiers, key) = super::parse_hotkey("Ctrl+Alt+1").unwrap();
        assert_eq!(modifiers, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT);
        assert_eq!(key, b'1' as u32);
        assert!(super::parse_hotkey("Ctrl+Alt").is_err());
        assert!(super::parse_hotkey("Ctrl+A+B").is_err());
        assert!(super::parse_hotkey("Ctrl+Nope").is_err());
    }

    #[test]
    fn unknown_keys_are_reported() {
        assert!(super::send_keys("Ctrl+Nope").unwrap_err().contains("Nope"));
        assert!(super::system("explode").is_err());
    }
}

/// Opens a text file for editing.
pub fn edit_file(path: &std::path::Path) {
    let editor = if cfg!(windows) { "notepad" } else { "xdg-open" };
    if let Err(err) = std::process::Command::new(editor).arg(path).spawn() {
        eprintln!("could not open {}: {err}", path.display());
    }
}
