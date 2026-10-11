//! The hub's icon in the Windows notification area, with Show, Restart and Quit, for
//! getting at the hub from the main screen. Clicking the icon also shows the hub.

use std::time::Duration;

use slint::{ComponentHandle, Timer, TimerMode};

use crate::{Activity, AppWindow};

/// Keeps the icon in the notification area; dropping this removes it.
pub struct Tray {
    #[cfg(windows)]
    _icon: tray_icon::TrayIcon,
    _poll: Timer,
}

/// Adds the icon. Returns None (after logging why) when Windows won't show it.
pub fn start(ui: slint::Weak<AppWindow>) -> Option<Tray> {
    imp::start(ui)
}

/// Brings the hub back in front, wakes its screen and leaves full-screen games alone.
fn show(ui: &AppWindow) {
    ui.global::<Activity>().set_idle_seconds(0);
    let _ = ui.show();
    #[cfg(windows)]
    {
        use slint::winit_030::{winit::window::WindowLevel, WinitWindowAccessor};
        ui.window().with_winit_window(|window| {
            window.set_minimized(false);
            // The hub never takes focus, so raise it by putting it on top for a moment.
            window.set_window_level(WindowLevel::AlwaysOnTop);
            window.set_window_level(WindowLevel::Normal);
        });
    }
}

/// Starts a fresh copy of the hub and closes this one.
fn restart() {
    match std::env::current_exe().and_then(|exe| std::process::Command::new(exe).spawn()) {
        Ok(_) => {
            let _ = slint::quit_event_loop();
        }
        Err(err) => eprintln!("tray: could not restart: {err}"),
    }
}

#[cfg(windows)]
mod imp {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    use super::*;

    pub fn start(ui: slint::Weak<AppWindow>) -> Option<Tray> {
        let show_item = MenuItem::new("Show hub", true, None);
        let restart_item = MenuItem::new("Restart", true, None);
        let quit_item = MenuItem::new("Quit", true, None);
        let menu = Menu::new();
        let built = menu.append_items(&[
            &show_item,
            &restart_item,
            &PredefinedMenuItem::separator(),
            &quit_item,
        ]);
        if let Err(err) = built {
            eprintln!("tray: could not build the menu: {err}");
            return None;
        }

        let icon = match load_icon() {
            Some(icon) => icon,
            None => {
                eprintln!("tray: could not read the icon");
                return None;
            }
        };
        let tray = TrayIconBuilder::new()
            .with_tooltip("PC Hub")
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build();
        let tray = match tray {
            Ok(tray) => tray,
            Err(err) => {
                eprintln!("tray: could not add the icon: {err}");
                return None;
            }
        };

        // Menu and click events arrive on channels; check them a few times a second.
        let (show_id, restart_id, quit_id) = (show_item.id().clone(), restart_item.id().clone(), quit_item.id().clone());
        let poll = Timer::default();
        poll.start(TimerMode::Repeated, Duration::from_millis(150), move || {
            let Some(ui) = ui.upgrade() else { return };
            while let Ok(event) = MenuEvent::receiver().try_recv() {
                if event.id == show_id {
                    show(&ui);
                } else if event.id == restart_id {
                    restart();
                } else if event.id == quit_id {
                    let _ = slint::quit_event_loop();
                }
            }
            while let Ok(event) = TrayIconEvent::receiver().try_recv() {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    show(&ui);
                }
            }
        });
        Some(Tray { _icon: tray, _poll: poll })
    }

    fn load_icon() -> Option<Icon> {
        let image = image::load_from_memory(include_bytes!("../ui/icon.png")).ok()?;
        let image = image.resize(64, 64, image::imageops::FilterType::Triangle).to_rgba8();
        let (width, height) = image.dimensions();
        Icon::from_rgba(image.into_raw(), width, height).ok()
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub fn start(_ui: slint::Weak<AppWindow>) -> Option<Tray> {
        let _ = (show, restart);
        None
    }
}
