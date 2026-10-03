// Hide the console window in release builds on Windows; keep it in debug for logs.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod actions;
mod audio;
mod config;
mod gpu;
mod hwinfo;
mod icons;
mod media;
mod settings;
mod stats;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, SystemTime};

use slint::{ComponentHandle, ModelRc, Timer, TimerMode, VecModel};

use audio::AudioCommand;
use media::MediaCommand;

slint::include_modules!();

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

fn main() -> Result<(), slint::PlatformError> {
    let (config, config_path) = config::load();
    let settings_path = settings::path_beside(config_path.as_deref());
    let ui = AppWindow::new()?;
    let hub = ui.global::<Hub>();

    // The accent from hub.toml, used unless one was picked on the hub.
    let default_accent = config.accent_color().unwrap_or(ui.global::<Palette>().get_accent());

    // Shortcuts: labels and icons go to the UI, the commands stay on the Rust side.
    let shortcuts = Rc::new(RefCell::new(Vec::new()));
    apply_shortcuts(&ui, &config, &shortcuts);
    hub.on_launch_shortcut({
        let weak = ui.as_weak();
        let shortcuts = shortcuts.clone();
        move |index| {
            let Some(shortcut) = shortcuts.borrow().get(index as usize).cloned() else {
                return;
            };
            let time = chrono::Local::now().format("%H:%M");
            let message = match shortcut.launch() {
                Ok(()) => format!("Last: {} · {time}", shortcut.label),
                Err(err) => {
                    eprintln!("shortcut \"{}\": {err}", shortcut.label);
                    format!("Could not run {}: {err}", shortcut.label)
                }
            };
            if let Some(ui) = weak.upgrade() {
                ui.global::<Hub>().set_last_action(message.into());
            }
        }
    });
    hub.on_edit_config({
        let path = config_path.clone();
        move || match &path {
            Some(path) => actions::edit_file(path),
            None => eprintln!("config: no hub.toml to edit"),
        }
    });
    let reload = watch_config(&ui, config_path.clone(), shortcuts.clone());

    // Media and audio run on their own threads because the Windows APIs block.
    let media = media::spawn(ui.as_weak());
    let media_command = |command: MediaCommand| {
        let media = media.clone();
        move || {
            let _ = media.send(command);
        }
    };
    hub.on_media_play_pause(media_command(MediaCommand::PlayPause));
    hub.on_media_next(media_command(MediaCommand::Next));
    hub.on_media_previous(media_command(MediaCommand::Previous));
    hub.on_media_shuffle(media_command(MediaCommand::ToggleShuffle));
    hub.on_media_repeat(media_command(MediaCommand::ToggleRepeat));

    let audio = audio::spawn(
        ui.as_weak(),
        audio::DeviceChoices {
            outputs: config.outputs.clone(),
            inputs: config.inputs.clone(),
        },
    );
    hub.on_set_volume({
        let audio = audio.clone();
        move |level| {
            let _ = audio.send(AudioCommand::SetVolume(level));
        }
    });
    hub.on_select_output({
        let audio = audio.clone();
        move |index| {
            let _ = audio.send(AudioCommand::SelectOutput(index.max(0) as usize));
        }
    });
    hub.on_select_input(move |index| {
        let _ = audio.send(AudioCommand::SelectInput(index.max(0) as usize));
    });

    // Hex keypad for the custom accent color
    hub.on_hex_edit(|draft, key| {
        let mut digits = draft.to_string();
        match key.as_str() {
            "back" => {
                digits.pop();
            }
            "clear" => digits.clear(),
            key if digits.len() < 6 && key.len() == 1 && key.chars().all(|c| c.is_ascii_hexdigit()) => {
                digits.push_str(&key.to_ascii_uppercase());
            }
            _ => {}
        }
        digits.into()
    });
    hub.on_hex_complete(|digits| digits.len() == 6 && config::parse_color(&digits).is_some());
    hub.on_hex_color(|digits| config::parse_color(&digits).unwrap_or_default());

    // Settings sheet and Customize hub screen
    let saved = settings::load(&settings_path);
    apply_settings(&ui, &saved, default_accent);
    hub.set_slide(settings::index_of(&settings::PAGES, &saved.start_page, 1) as i32);
    hub.on_settings_changed({
        let weak = ui.as_weak();
        let path = settings_path.clone();
        move || {
            let Some(ui) = weak.upgrade() else { return };
            settings::save(&path, &read_settings(&ui, default_accent));
        }
    });
    hub.on_reset_settings({
        let weak = ui.as_weak();
        move || {
            let Some(ui) = weak.upgrade() else { return };
            let defaults = settings::Settings::default();
            apply_settings(&ui, &defaults, default_accent);
            settings::save(&settings_path, &defaults);
        }
    });
    hub.on_close_hub(|| {
        let _ = slint::quit_event_loop();
    });

    // The clock ticks every second; hardware readings follow the refresh interval setting.
    // The first readings arrive on the first tick: load needs two samples a moment apart.
    let mut monitor = Monitor::new(config.sensors.cpu_fan.clone());
    hub.set_system_summary(monitor.summary().into());
    update_clock(&ui);
    let tick = Timer::default();
    let weak = ui.as_weak();
    let mut seconds = 0u64;
    tick.start(TimerMode::Repeated, Duration::from_secs(1), move || {
        let Some(ui) = weak.upgrade() else { return };
        seconds += 1;
        update_clock(&ui);
        let hub = ui.global::<Hub>();
        hub.set_pc_locked(hub.get_dark_when_locked() && actions::session_locked());
        let every = hub.get_refresh_seconds().max(1) as u64;
        if seconds % every == 0 {
            monitor.refresh(&ui);
        }
    });

    ui.show()?;
    place_window(&ui, &config.window);
    actions::make_non_activating(ui.window());
    slint::run_event_loop()?;
    drop(reload);
    Ok(())
}

fn apply_settings(ui: &AppWindow, s: &settings::Settings, default_accent: slint::Color) {
    let accent = s
        .accent
        .as_deref()
        .and_then(config::parse_color)
        .unwrap_or(default_accent);
    ui.global::<Palette>().set_accent(accent);

    let hub = ui.global::<Hub>();
    let custom = s.custom_accent.as_deref().filter(|c| config::parse_color(c).is_some());
    hub.set_custom_accent(custom.map(|c| c.trim().trim_start_matches('#').to_uppercase()).unwrap_or_default().into());
    hub.set_clock_24h(s.clock_24h);
    hub.set_show_date(s.show_date);
    hub.set_show_uptime(s.show_uptime);
    hub.set_show_music(s.show_music);
    hub.set_show_monitor(s.show_monitor);
    hub.set_show_shortcuts(s.show_shortcuts);
    hub.set_start_page(settings::index_of(&settings::PAGES, &s.start_page, 1) as i32);
    hub.set_swipe(s.swipe);
    hub.set_slide_animation(settings::index_of(&settings::ANIMATIONS, &s.slide_animation, 2) as i32);
    hub.set_refresh_seconds(s.refresh_seconds as i32);
    hub.set_fahrenheit(s.fahrenheit);
    hub.set_heat_warning(s.heat_warning as i32);
    hub.set_show_memory(s.show_memory);
    hub.set_show_video_memory(s.show_video_memory);
    hub.set_show_network(s.show_network);
    hub.set_show_storage(s.show_storage);
    hub.set_show_album_art(s.show_album_art);
    hub.set_show_outputs(s.show_outputs);
    hub.set_show_inputs(s.show_inputs);
    hub.set_show_key_hints(s.show_key_hints);
    hub.set_brightness(s.brightness as i32);
    hub.set_idle_dim(s.dim_when_idle);
    hub.set_idle_minutes(s.idle_minutes as i32);
    hub.set_dark_when_locked(s.dark_when_locked);
    update_clock(ui);
}

fn read_settings(ui: &AppWindow, default_accent: slint::Color) -> settings::Settings {
    let hub = ui.global::<Hub>();
    let accent = ui.global::<Palette>().get_accent();
    let page = |i: i32| settings::PAGES[i.clamp(0, 2) as usize].to_string();
    settings::Settings {
        accent: (accent != default_accent).then(|| {
            format!("#{:02X}{:02X}{:02X}", accent.red(), accent.green(), accent.blue())
        }),
        custom_accent: Some(hub.get_custom_accent().to_string())
            .filter(|c| !c.is_empty())
            .map(|c| format!("#{c}")),
        clock_24h: hub.get_clock_24h(),
        show_date: hub.get_show_date(),
        show_uptime: hub.get_show_uptime(),
        show_music: hub.get_show_music(),
        show_monitor: hub.get_show_monitor(),
        show_shortcuts: hub.get_show_shortcuts(),
        start_page: page(hub.get_start_page()),
        swipe: hub.get_swipe(),
        slide_animation: settings::ANIMATIONS[hub.get_slide_animation().clamp(0, 2) as usize].into(),
        refresh_seconds: hub.get_refresh_seconds().max(1) as u32,
        fahrenheit: hub.get_fahrenheit(),
        heat_warning: hub.get_heat_warning().max(0) as u32,
        show_memory: hub.get_show_memory(),
        show_video_memory: hub.get_show_video_memory(),
        show_network: hub.get_show_network(),
        show_storage: hub.get_show_storage(),
        show_album_art: hub.get_show_album_art(),
        show_outputs: hub.get_show_outputs(),
        show_inputs: hub.get_show_inputs(),
        show_key_hints: hub.get_show_key_hints(),
        brightness: hub.get_brightness().max(0) as u32,
        dim_when_idle: hub.get_idle_dim(),
        idle_minutes: hub.get_idle_minutes().max(1) as u32,
        dark_when_locked: hub.get_dark_when_locked(),
    }
    .sanitized()
}

fn apply_shortcuts(
    ui: &AppWindow,
    config: &config::Config,
    shortcuts: &Rc<RefCell<Vec<config::ShortcutConfig>>>,
) {
    let items: Vec<Shortcut> = config
        .shortcuts
        .iter()
        .map(|s| Shortcut {
            label: s.label.as_str().into(),
            hint: s.hint().into(),
            glyph: icons::glyph(s.icon.as_deref()).into(),
        })
        .collect();
    let hub = ui.global::<Hub>();
    hub.set_shortcuts(ModelRc::from(Rc::new(VecModel::from(items))));
    hub.set_shortcut_columns(config.columns.max(1) as i32);
    *shortcuts.borrow_mut() = config.shortcuts.clone();
}

/// Picks up shortcut edits in hub.toml without a restart.
fn watch_config(
    ui: &AppWindow,
    path: Option<PathBuf>,
    shortcuts: Rc<RefCell<Vec<config::ShortcutConfig>>>,
) -> Timer {
    let timer = Timer::default();
    let Some(path) = path else { return timer };
    let modified = |path: &PathBuf| std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let mut last: Option<SystemTime> = modified(&path);
    let weak = ui.as_weak();
    timer.start(TimerMode::Repeated, Duration::from_secs(2), move || {
        let now = modified(&path);
        if now == last {
            return;
        }
        last = now;
        let Some(ui) = weak.upgrade() else { return };
        match config::read(&path) {
            Ok(config) => {
                apply_shortcuts(&ui, &config, &shortcuts);
                ui.global::<Hub>().set_last_action("Shortcuts reloaded from hub.toml".into());
            }
            Err(err) => {
                eprintln!("config: {} has an error:\n{err}", path.display());
                ui.global::<Hub>().set_last_action("hub.toml has an error; see the console".into());
            }
        }
    });
    timer
}

fn update_clock(ui: &AppWindow) {
    let hub = ui.global::<Hub>();
    let now = chrono::Local::now();
    let format = if hub.get_clock_24h() { "%H:%M" } else { "%-I:%M %p" };
    hub.set_clock(now.format(format).to_string().into());
    hub.set_date_text(now.format("%a %-d %b").to_string().into());
}

struct Monitor {
    stats: stats::Stats,
    gpu: gpu::Gpu,
    cpu_fan: Option<String>,
}

impl Monitor {
    fn new(cpu_fan: Option<String>) -> Self {
        Self {
            stats: stats::Stats::new(),
            gpu: gpu::Gpu::new(),
            cpu_fan,
        }
    }

    /// e.g. "Ryzen 7 7800X3D · RTX 5070 · 32 GB RAM"
    fn summary(&self) -> String {
        let mut parts = vec![self.stats.cpu_name()];
        parts.extend(self.gpu.name());
        parts.push(format!("{} GB RAM", self.stats.ram_total_gb()));
        parts.retain(|p| !p.is_empty());
        parts.join(" · ")
    }

    fn refresh(&mut self, ui: &AppWindow) {
        let hub = ui.global::<Hub>();
        let fahrenheit = hub.get_fahrenheit();
        let warning = hub.get_heat_warning() as f32;
        let temp = |celsius: Option<f32>| match celsius {
            Some(c) if fahrenheit => format!("{:.0}°F", c * 9.0 / 5.0 + 32.0),
            Some(c) => format!("{c:.0}°C"),
            None => "—".into(),
        };
        let or_dash = |value: Option<String>| value.unwrap_or_else(|| "—".into());

        let sample = self.stats.sample();
        hub.set_uptime_text(format!("Up {}", stats::format_uptime(sample.uptime_secs)).into());

        // Processor: load from Windows, the rest from HWiNFO when it's running.
        let sensors = hwinfo::read(self.cpu_fan.as_deref());
        hub.set_cpu_sensors_missing(sensors.is_none());
        let sensors = sensors.unwrap_or_default();
        hub.set_cpu_load(sample.cpu);
        hub.set_cpu_temp(temp(sensors.temp_c).into());
        hub.set_cpu_temp_hot(sensors.temp_c.is_some_and(|t| t > warning));
        let mhz = sensors.clock_mhz.or((sample.cpu_mhz > 0).then_some(sample.cpu_mhz as f32));
        hub.set_cpu_clock(or_dash(mhz.map(|m| format!("{:.2}", m / 1000.0))).into());
        hub.set_cpu_power(or_dash(sensors.power_w.map(|w| format!("{w:.0}"))).into());
        hub.set_cpu_fan(or_dash(sensors.fan_rpm.map(|r| thousands(r as u64))).into());

        // Graphics card
        let gpu = self.gpu.sample();
        let g = gpu.as_ref();
        hub.set_gpu_load(g.map(|g| g.load).unwrap_or(0.0));
        hub.set_gpu_temp(temp(g.and_then(|g| g.temp_c)).into());
        hub.set_gpu_temp_hot(g.and_then(|g| g.temp_c).is_some_and(|t| t > warning));
        hub.set_gpu_clock(or_dash(g.and_then(|g| g.clock_mhz).map(|c| thousands(c as u64))).into());
        hub.set_gpu_power(or_dash(g.and_then(|g| g.power_w).map(|w| format!("{w:.0}"))).into());
        let fan = g.and_then(|g| g.fan);
        hub.set_gpu_fan(or_dash(fan.map(|(v, _)| thousands(v as u64))).into());
        hub.set_gpu_fan_unit(if fan.is_some_and(|(_, rpm)| !rpm) { "%" } else { "RPM" }.into());

        // Memory, video memory, network, disk
        hub.set_ram_value(format!("{:.1}", sample.ram_used as f64 / GIB).into());
        hub.set_ram_unit(format!(" / {} GB", self.stats.ram_total_gb()).into());
        hub.set_ram_usage(fraction(sample.ram_used, sample.ram_total));

        match g.and_then(|g| g.vram) {
            Some((used, total)) => {
                hub.set_vram_value(format!("{:.1}", used as f64 / GIB).into());
                hub.set_vram_unit(format!(" / {:.0} GB", total as f64 / GIB).into());
                hub.set_vram_usage(fraction(used, total));
            }
            None => hub.set_vram_value("—".into()),
        }

        hub.set_net_down(format!("{:.1}", sample.net_down / 1e6).into());
        hub.set_net_up(format!("{:.1}", sample.net_up / 1e6).into());

        if let Some((used, total)) = sample.disk {
            let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
            hub.set_disk_label(format!("STORAGE {drive}").into());
            hub.set_disk_value(format!("{:.0}", used as f64 / GIB).into());
            hub.set_disk_unit(format!(" / {:.0} GB", total as f64 / GIB).into());
            hub.set_disk_usage(fraction(used, total));
        }
    }
}

fn fraction(used: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (used as f64 / total as f64) as f32
    }
}

/// 1450 -> "1,450"
fn thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn place_window(ui: &AppWindow, window: &config::WindowConfig) {
    if let (Some(x), Some(y)) = (window.x, window.y) {
        ui.window().set_position(slint::PhysicalPosition::new(x, y));
    }
    if window.fullscreen {
        ui.window().set_fullscreen(true);
    }
}
