//! The hub screen's real backlight, through DDC/CI (the monitor's own brightness setting,
//! changed over the video cable). Many monitors support it; some turn it off in their
//! menu, and laptop panels and some USB screens don't have it. Talking to a monitor takes
//! tens of milliseconds, so it happens on a thread of its own.

use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

/// What the backlight thread was asked to do.
enum Request {
    /// Set the monitor showing this window (by its handle) to a percentage.
    Set { window: isize, percent: u32 },
}

/// Sends brightness changes to the monitor in the background, skipping ones overtaken by
/// a newer change.
pub struct Backlight {
    requests: Sender<Request>,
    /// Whether the last attempt worked: None before any, then Ok or why not.
    pub status: Arc<Mutex<Option<Result<(), String>>>>,
    last: std::cell::Cell<Option<(isize, u32)>>,
}

impl Backlight {
    pub fn start() -> Self {
        let (requests, received) = mpsc::channel::<Request>();
        let status = Arc::new(Mutex::new(None));
        let shared = status.clone();
        std::thread::Builder::new()
            .name("backlight".into())
            .spawn(move || {
                while let Ok(mut request) = received.recv() {
                    // Only the newest request matters.
                    while let Ok(newer) = received.try_recv() {
                        request = newer;
                    }
                    let Request::Set { window, percent } = request;
                    let result = set(window, percent);
                    if let Err(err) = &result {
                        eprintln!("backlight: {err}");
                    }
                    if let Ok(mut status) = shared.lock() {
                        *status = Some(result);
                    }
                }
            })
            .expect("failed to start the backlight thread");
        Self {
            requests,
            status,
            last: std::cell::Cell::new(None),
        }
    }

    /// Sets the backlight of the monitor showing `window` (its native handle), unless it
    /// was already set to that.
    pub fn set(&self, window: isize, percent: u32) {
        let percent = percent.min(100);
        if self.last.get() == Some((window, percent)) {
            return;
        }
        self.last.set(Some((window, percent)));
        let _ = self.requests.send(Request::Set { window, percent });
    }

    /// Forgets the last value, so the next set goes to the monitor even if it's the same
    /// (after the hub moved to another screen, say).
    pub fn forget(&self) {
        self.last.set(None);
    }

    /// None until tried; then whether the monitor took the change.
    pub fn works(&self) -> Option<Result<(), String>> {
        self.status.lock().ok().and_then(|s| s.clone())
    }
}

/// Sets the backlight now, waiting for the monitor (for when the hub closes).
pub fn set_now(window: isize, percent: u32) -> Result<(), String> {
    set(window, percent.min(100))
}

#[cfg(windows)]
fn set(window: isize, percent: u32) -> Result<(), String> {
    use windows::Win32::Devices::Display::{
        DestroyPhysicalMonitors, GetMonitorBrightness, GetNumberOfPhysicalMonitorsFromHMONITOR,
        GetPhysicalMonitorsFromHMONITOR, SetMonitorBrightness, PHYSICAL_MONITOR,
    };
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTONEAREST};

    unsafe {
        let monitor = MonitorFromWindow(HWND(window as *mut _), MONITOR_DEFAULTTONEAREST);
        let mut count = 0u32;
        GetNumberOfPhysicalMonitorsFromHMONITOR(monitor, &mut count).map_err(|e| e.to_string())?;
        if count == 0 {
            return Err("this screen has no monitor that can be controlled".into());
        }
        let mut monitors = vec![PHYSICAL_MONITOR::default(); count as usize];
        GetPhysicalMonitorsFromHMONITOR(monitor, &mut monitors).map_err(|e| e.to_string())?;
        let mut result = Err("the monitor doesn't support DDC/CI brightness (check its menu)".to_string());
        for physical in &monitors {
            let (mut min, mut current, mut max) = (0u32, 0u32, 0u32);
            if GetMonitorBrightness(physical.hPhysicalMonitor, &mut min, &mut current, &mut max) == 0 || max <= min {
                continue;
            }
            let value = min + (max - min) * percent / 100;
            if SetMonitorBrightness(physical.hPhysicalMonitor, value) != 0 {
                result = Ok(());
            }
        }
        let _ = DestroyPhysicalMonitors(&monitors);
        result
    }
}

#[cfg(not(windows))]
fn set(_window: isize, _percent: u32) -> Result<(), String> {
    Err("only supported on Windows".into())
}
