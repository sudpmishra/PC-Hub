//! Key combinations on the main keyboard that switch the hub's page from any app, set
//! under [hotkeys] in hub.toml. Windows delivers them to a thread of their own.

use slint::ComponentHandle;

use crate::{Activity, AppWindow, Hub};

/// Registered hotkeys; dropping this unregisters them.
pub struct Hotkeys {
    #[cfg(windows)]
    thread_id: u32,
    #[cfg(windows)]
    thread: Option<std::thread::JoinHandle<()>>,
}

/// Registers `bindings` of (key combination, page index). Combinations that can't be
/// parsed or are already taken by another app are reported and skipped.
pub fn start(bindings: Vec<(String, i32)>, ui: slint::Weak<AppWindow>) -> Option<Hotkeys> {
    if bindings.is_empty() {
        return None;
    }
    imp::start(bindings, ui)
}

/// Shows the page and wakes the screen if it was dimmed.
fn show_page(ui: &slint::Weak<AppWindow>, page: i32) {
    let _ = ui.upgrade_in_event_loop(move |ui| {
        ui.global::<Activity>().set_idle_seconds(0);
        ui.global::<Hub>().set_slide(page);
    });
}

#[cfg(windows)]
mod imp {
    use std::sync::mpsc;

    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetMessageW, PeekMessageW, PostThreadMessageW, MSG, PM_NOREMOVE, WM_HOTKEY, WM_QUIT, WM_USER,
    };

    use super::*;
    use crate::actions;

    pub fn start(bindings: Vec<(String, i32)>, ui: slint::Weak<AppWindow>) -> Option<Hotkeys> {
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new().name("hotkeys".into()).spawn(move || {
            // The id of each registered hotkey is its index in `pages`.
            let mut pages = Vec::new();
            for (spec, page) in bindings {
                let registered = actions::parse_hotkey(&spec).and_then(|(modifiers, key)| {
                    unsafe { RegisterHotKey(None, pages.len() as i32, modifiers, key) }
                        .map_err(|_| "already used by another app".to_string())
                });
                match registered {
                    Ok(()) => pages.push(page),
                    Err(err) => eprintln!("hotkeys: \"{spec}\": {err}"),
                }
            }
            // Make sure this thread has a message queue (even if nothing registered), so
            // it can be told to quit.
            let mut msg = MSG::default();
            let _ = unsafe { PeekMessageW(&mut msg, None, WM_USER, WM_USER, PM_NOREMOVE) };
            let _ = tx.send(unsafe { GetCurrentThreadId() });
            while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
                if msg.message == WM_HOTKEY {
                    if let Some(&page) = pages.get(msg.wParam.0) {
                        show_page(&ui, page);
                    }
                }
            }
            for id in 0..pages.len() {
                let _ = unsafe { UnregisterHotKey(None, id as i32) };
            }
        });
        let thread = match spawned {
            Ok(thread) => thread,
            Err(err) => {
                eprintln!("hotkeys: could not start: {err}");
                return None;
            }
        };
        rx.recv().ok().map(|thread_id| Hotkeys {
            thread_id,
            thread: Some(thread),
        })
    }

    impl Drop for Hotkeys {
        fn drop(&mut self) {
            let _ = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
            // Wait until they are unregistered, so new ones with the same keys can be registered.
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub fn start(_bindings: Vec<(String, i32)>, _ui: slint::Weak<AppWindow>) -> Option<Hotkeys> {
        let _ = show_page;
        None
    }
}
