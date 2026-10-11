// Hide the console window in release builds on Windows; keep it in debug for logs.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod actions;
mod appicons;
mod audio;
mod backlight;
mod backup;
mod calendar;
mod clipboard;
mod commands;
mod config;
mod devices;
mod discord;
mod gpu;
mod history;
mod hotkeys;
mod hwinfo;
mod icons;
mod media;
mod network;
mod notifications;
mod obs;
mod profiles;
mod rtss;
mod settings;
mod stats;
mod toggles;
mod tray;
mod web;
mod window;

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use slint::{ComponentHandle, Model, ModelRc, PhysicalPosition, PhysicalSize, Timer, TimerMode, VecModel};

use audio::AudioCommand;
use history::History;
use media::MediaCommand;

slint::include_modules!();

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

fn main() -> Result<(), slint::PlatformError> {
    start_log();
    let (config, config_path) = config::load();
    let settings_path = settings::path_beside(config_path.as_deref());
    let saved = settings::load(&settings_path);
    let ui = AppWindow::new()?;
    let hub = ui.global::<Hub>();

    // Media and audio run on their own threads because the Windows APIs block.
    let audio = audio::spawn(ui.as_weak(), device_choices(&config));
    let media = media::spawn(ui.as_weak());
    let config_now = config.clone();
    let token_file = settings_path.with_file_name("discord-token.json");

    let app = Rc::new(App {
        ui: ui.as_weak(),
        builtin_accent: ui.global::<Palette>().get_chosen(),
        config_stamp: Cell::new(config_path.as_deref().and_then(modified)),
        config_path,
        settings_path,
        config: RefCell::new(config),
        geometry: Cell::new(saved.window),
        screen: Cell::new(saved.screen),
        audio: audio.clone(),
        tiles_generation: Arc::new(AtomicU64::new(0)),
        running_commands: Arc::new(Mutex::new(HashSet::new())),
        hotkeys: RefCell::new(None),
        shortcut_page: RefCell::new(saved.shortcut_page.clone()),
        weather: Arc::new(Mutex::new(None)),
        weather_due: Cell::new(true),
        weather_place: RefCell::new(saved.weather_place.clone()),
        latency: Arc::new(Mutex::new(None)),
        update: Arc::new(Mutex::new(None)),
        tray: RefCell::new(None),
        toggles: toggles::watch(),
        clipboard: RefCell::new(clipboard::History::default()),
        notifications: notifications::start(config_now.notifications.apps.clone()),
        discord: discord::start(config_now.discord.clone(), token_file),
        obs: obs::start(config_now.obs()),
        calendar: Arc::new(Mutex::new(None)),
        calendar_due: Cell::new(true),
        batteries: Arc::new(Mutex::new(Vec::new())),
        backlight: backlight::Backlight::start(),
        mic_meter: Arc::new(AtomicBool::new(false)),
    });

    apply_settings(&ui, &saved, app.defaults());
    hub.set_slide(settings::index_of(&settings::PAGES, &saved.start_page, 1) as i32);
    app.apply_shortcuts(&ui);
    app.start_hotkeys();

    connect_shortcuts(&ui, &app);
    connect_media(&ui, media);
    connect_audio(&ui, audio);
    connect_settings(&ui, &app);
    connect_window(&ui, &app);
    connect_tile_editor(&ui, &app);
    connect_backups(&ui, &app);
    connect_widgets(&ui, &app);
    connect_profiles(&ui, &app);
    audio::spawn_meter(ui.as_weak(), app.mic_meter.clone());
    let reload = watch_config(&app);
    let tick = start_ticking(&ui, &app);

    ui.show()?;
    window::place(ui.window(), &app.placement(hub.get_fullscreen()));
    actions::make_non_activating(ui.window());
    *app.tray.borrow_mut() = tray::start(ui.as_weak());
    // The native window only exists once the event loop runs.
    Timer::single_shot(Duration::from_millis(300), {
        let app = app.clone();
        move || {
            let Some(ui) = app.ui.upgrade() else { return };
            actions::make_non_activating(ui.window());
            // Opening on a screen with another scale, Windows rescales the window after it
            // was sized; size it again so it reopens at the size it was left.
            if let (false, Some([_, _, width, height])) = (ui.window().is_fullscreen(), app.geometry.get()) {
                ui.window().set_size(PhysicalSize::new(width as u32, height as u32));
            }
            let handler = app.clone();
            window::on_file_events(ui.window(), move |event| handler.file_event(event));
        }
    });
    slint::run_event_loop()?;
    // Leave the monitor at the chosen brightness rather than dimmed.
    if app.backlight.works() == Some(Ok(())) {
        if let Some(window) = actions::native_handle(ui.window()) {
            let _ = backlight::set_now(window, ui.global::<Hub>().get_brightness().max(0) as u32);
        }
    }
    drop((reload, tick));
    Ok(())
}

/// Release builds have no console, so their messages go to pc-hub.log beside the .exe.
fn start_log() {
    if cfg!(debug_assertions) {
        return;
    }
    let Some(path) = std::env::current_exe().ok().map(|exe| exe.with_file_name("pc-hub.log")) else {
        return;
    };
    // Start the file over once it gets big.
    let keep = std::fs::metadata(&path).is_ok_and(|m| m.len() < 1_000_000);
    let mut options = std::fs::OpenOptions::new();
    if keep {
        options.append(true).create(true);
    } else {
        options.write(true).create(true).truncate(true);
    }
    if let Ok(file) = options.open(&path) {
        actions::log_to_file(file);
        eprintln!(
            "--- PC Hub {} started {}",
            env!("CARGO_PKG_VERSION"),
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        );
    }
}

/// State the UI callbacks share.
struct App {
    ui: slint::Weak<AppWindow>,
    /// The accent built into the UI, for when hub.toml doesn't set one.
    builtin_accent: slint::Color,
    config_path: Option<PathBuf>,
    settings_path: PathBuf,
    /// hub.toml as last loaded.
    config: RefCell<config::Config>,
    /// When hub.toml was last seen changed. The hub's own edits update it, so they
    /// aren't reloaded as if made elsewhere.
    config_stamp: Cell<Option<SystemTime>>,
    /// Where the window was left out of full screen: x, y, width, height.
    geometry: Cell<Option<[i32; 4]>>,
    /// The screen picked on the hub for full screen: its top-left corner.
    screen: Cell<Option<[i32; 2]>>,
    audio: Sender<AudioCommand>,
    /// Counts reloads of the tiles, so icons and command output found for an older set
    /// of tiles aren't put on the new ones.
    tiles_generation: Arc<AtomicU64>,
    /// Commands of command tiles that are running, so a slow one isn't started twice.
    running_commands: Arc<Mutex<HashSet<String>>>,
    hotkeys: RefCell<Option<hotkeys::Hotkeys>>,
    /// The shortcut page shown, by name, when hub.toml has pages.
    shortcut_page: RefCell<Option<String>>,
    /// The latest weather lookup, or why it failed, written by a background thread.
    weather: Arc<Mutex<Option<Result<web::Weather, String>>>>,
    /// Set when the weather should be looked up again soon.
    weather_due: Cell<bool>,
    /// A place for the weather typed on the hub; None uses [weather] in hub.toml.
    weather_place: RefCell<Option<String>>,
    /// The latest latency reading for the network card, written by a background thread.
    latency: Arc<Mutex<Option<Result<Duration, String>>>>,
    /// The latest release found on GitHub (version and page), None if there are none yet.
    update: Arc<Mutex<Option<UpdateCheck>>>,
    tray: RefCell<Option<tray::Tray>>,
    /// Wi-Fi, Bluetooth, night light and Do Not Disturb, for tiles that light up.
    toggles: Arc<Mutex<toggles::States>>,
    /// Text copied lately, when clipboard history is on.
    clipboard: RefCell<clipboard::History>,
    notifications: (Sender<notifications::Command>, notifications::Latest),
    discord: (Sender<discord::Command>, discord::Shared),
    obs: (Sender<obs::Command>, obs::Shared),
    /// The latest calendar download: events and errors.
    calendar: Arc<Mutex<Option<CalendarResult>>>,
    calendar_due: Cell<bool>,
    /// Wireless devices' batteries, read every minute in the background.
    batteries: Arc<Mutex<Vec<devices::DeviceBattery>>>,
    backlight: backlight::Backlight,
    /// Set while the microphone meter should measure.
    mic_meter: Arc<AtomicBool>,
}

type CalendarResult = (Vec<calendar::Event>, Vec<String>);

type UpdateCheck = Result<Option<(String, String)>, String>;

/// Choices hub.toml makes, which a choice saved on the hub replaces.
#[derive(Clone, Copy)]
struct Defaults {
    accent: slint::Color,
    fullscreen: bool,
}

impl App {
    fn defaults(&self) -> Defaults {
        let config = self.config.borrow();
        Defaults {
            accent: config.accent_color().unwrap_or(self.builtin_accent),
            fullscreen: config.window.fullscreen,
        }
    }

    /// Where the window goes: full screen on the screen picked on the hub (or at
    /// hub.toml's corner), or a window where it was left.
    fn placement(&self, fullscreen: bool) -> window::Placement {
        let window = &self.config.borrow().window;
        window::Placement {
            fullscreen,
            geometry: self.geometry.get(),
            screen: self.screen.get().or(window.x.zip(window.y).map(|(x, y)| [x, y])),
        }
    }

    /// The folder hub.toml and the hub's settings are in.
    fn config_dir(&self) -> PathBuf {
        self.settings_path
            .parent()
            .map(Path::to_path_buf)
            .filter(|dir| !dir.as_os_str().is_empty())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// Lists the newest backups in Customize hub.
    fn refresh_backups(&self, ui: &AppWindow) {
        let rows: Vec<Choice> = backup::list(&self.config_dir())
            .into_iter()
            .take(5)
            .map(|name| Choice {
                label: format!("Backup from {}", backup::describe(&name)).into(),
                name: name.into(),
            })
            .collect();
        ui.global::<Hub>().set_backups(ModelRc::from(Rc::new(VecModel::from(rows))));
    }

    /// Reads hub.toml and the hub's settings again, after a backup was restored.
    fn reload_everything(&self, ui: &AppWindow) {
        if let Some(path) = self.config_path.as_deref() {
            match config::read(path) {
                Ok(config) => {
                    self.config_stamp.set(modified(path));
                    self.apply_config(ui, config);
                }
                Err(err) => eprintln!("backup: the restored hub.toml has an error: {err}"),
            }
        }
        let saved = settings::load(&self.settings_path);
        self.geometry.set(saved.window);
        self.screen.set(saved.screen);
        *self.weather_place.borrow_mut() = saved.weather_place.clone();
        *self.shortcut_page.borrow_mut() = saved.shortcut_page.clone();
        apply_settings(ui, &saved, self.defaults());
        window::place(ui.window(), &self.placement(ui.global::<Hub>().get_fullscreen()));
        self.refresh_weather();
        self.apply_shortcuts(ui);
    }

    /// Where the weather is for: the place typed on the hub, or else hub.toml's [weather].
    fn weather_config(&self) -> config::WeatherConfig {
        match self.weather_place.borrow().as_deref() {
            Some(place) => config::WeatherConfig {
                place: Some(place.to_string()),
                ..Default::default()
            },
            None => self.config.borrow().weather.clone(),
        }
    }

    /// Forgets the last weather and looks it up again on the next tick.
    fn refresh_weather(&self) {
        if let Ok(mut latest) = self.weather.lock() {
            *latest = None;
        }
        self.weather_due.set(true);
    }

    fn save_settings(&self, ui: &AppWindow) {
        let mut settings = read_settings(ui, self.defaults(), self.geometry.get(), self.screen.get());
        settings.shortcut_page = self.shortcut_page.borrow().clone();
        settings.weather_place = self.weather_place.borrow().clone();
        settings::save(&self.settings_path, &settings);
    }

    /// Sends the tiles to the UI; the commands behind them stay in `config`. Then looks
    /// up app icons and runs the command and web tiles that refresh by themselves.
    fn apply_shortcuts(&self, ui: &AppWindow) {
        let generation = self.tiles_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let config = self.config.borrow();

        // The shortcut page shown: the one picked last, while it still exists.
        let pages = config.page_names();
        let page = {
            let wanted = self.shortcut_page.borrow();
            pages
                .iter()
                .position(|p| wanted.as_deref().is_some_and(|w| w.eq_ignore_ascii_case(p)))
                .unwrap_or(0)
        };
        let page_name = pages.get(page).map(String::as_str);

        let all: Vec<Shortcut> = config
            .shortcuts
            .iter()
            .enumerate()
            .map(|(i, s)| Shortcut {
                index: i as i32,
                label: s.label.as_str().into(),
                hint: s.hint().into(),
                glyph: icons::glyph(s.icon_name()).into(),
                toggle: s.toggle().into(),
                glyph_on: icons::glyph_on(s.icon_name()).into(),
                has_image: false,
                image: slint::Image::default(),
                has_hold: s.hold.is_some(),
                is_folder: s.folder.is_some(),
            })
            .collect();
        // Tiles inside a folder show when it opens, not on the page.
        let shown: Vec<Shortcut> = all
            .iter()
            .filter(|tile| {
                let shortcut = &config.shortcuts[tile.index as usize];
                shortcut.on_page(page_name) && shortcut.in_folder().is_none()
            })
            .cloned()
            .collect();
        let hub = ui.global::<Hub>();
        // Tiles may have moved, so a pending "tap again" would point at the wrong one.
        hub.set_armed_shortcut(-1);
        let names: Vec<slint::SharedString> = pages.iter().map(|p| p.as_str().into()).collect();
        hub.set_shortcut_pages(ModelRc::from(Rc::new(VecModel::from(names))));
        hub.set_shortcut_page(page as i32);
        hub.set_shortcuts(ModelRc::from(Rc::new(VecModel::from(shown))));
        hub.set_all_shortcuts(ModelRc::from(Rc::new(VecModel::from(all))));
        hub.set_shortcut_columns(config.columns.max(1) as i32);

        // Tiles that open a file, folder or app without an icon set get its real icon.
        let targets: Vec<(usize, PathBuf)> = config
            .shortcuts
            .iter()
            .enumerate()
            .filter(|(_, s)| s.icon.is_none() && s.keys.is_none() && s.action.is_none() && s.work().is_none())
            .filter_map(|(i, s)| appicons::target_file(&s.run).map(|file| (i, file)))
            .collect();
        let (weak, tiles) = (self.ui.clone(), self.tiles_generation.clone());
        appicons::load(targets, move |index, pixels| {
            let tiles = tiles.clone();
            let _ = weak.upgrade_in_event_loop(move |ui| {
                let image = slint::Image::from_rgba8_premultiplied(pixels);
                update_tile(&ui, &tiles, generation, index, |tile| {
                    tile.image = image.clone();
                    tile.has_image = true;
                });
            });
        });

        let refreshing: Vec<usize> = config
            .shortcuts
            .iter()
            .enumerate()
            .filter(|(_, s)| s.refresh_secs().is_some())
            .map(|(i, _)| i)
            .collect();
        drop(config);
        for index in refreshing {
            self.run_work(index, false);
        }
    }

    /// Shows another shortcut page. `remember` keeps it for next time (a page picked by
    /// hand, not one that turned on by itself for a running app).
    fn show_shortcut_page(&self, ui: &AppWindow, name: Option<String>, remember: bool) {
        *self.shortcut_page.borrow_mut() = name;
        self.apply_shortcuts(ui);
        if remember {
            self.save_settings(ui);
        }
    }

    /// Does a command, web or macro tile's work in the background. The result replaces
    /// the tile's hint and, with `announce`, shows under the Shortcuts title too.
    fn run_work(&self, index: usize, announce: bool) {
        let Some(shortcut) = self.config.borrow().shortcuts.get(index).cloned() else { return };
        let Some(work) = shortcut.work() else { return };
        // Don't start a tile again while it's still busy (a slow server, a long macro).
        let key = format!("{index}:{}", shortcut.label);
        let started = self.running_commands.lock().is_ok_and(|mut running| running.insert(key.clone()));
        if !started {
            return;
        }
        let generation = self.tiles_generation.load(Ordering::SeqCst);
        let (weak, tiles, running) = (self.ui.clone(), self.tiles_generation.clone(), self.running_commands.clone());
        let audio = self.audio.clone();
        std::thread::spawn(move || {
            let result = commands::run_work(&work, &audio);
            if let Ok(mut running) = running.lock() {
                running.remove(&key);
            }
            let _ = weak.upgrade_in_event_loop(move |ui| {
                let text = result.unwrap_or_else(|err| {
                    eprintln!("tile \"{}\": {err}", shortcut.label);
                    format!("Error: {err}")
                });
                update_tile(&ui, &tiles, generation, index, |tile| tile.hint = text.as_str().into());
                if announce {
                    ui.global::<Hub>().set_last_action(format!("{}: {text}", shortcut.label).into());
                }
            });
        });
    }

    /// Registers the hotkeys from hub.toml, replacing any registered before.
    fn start_hotkeys(&self) {
        // Unregister the old ones first, so the same keys can be registered again.
        drop(self.hotkeys.borrow_mut().take());
        let bindings = self.config.borrow().hotkeys.bindings();
        *self.hotkeys.borrow_mut() = hotkeys::start(bindings, self.ui.clone());
    }

    /// Applies a newly read hub.toml: tiles, columns, accent, devices, sensors and window.
    /// Choices made on the hub keep winning over it.
    fn apply_config(&self, ui: &AppWindow, new: config::Config) {
        let before = self.defaults();
        let old = self.config.replace(new);
        let now = self.defaults();
        let hub = ui.global::<Hub>();

        // The accent and full screen follow hub.toml unless something else was picked on
        // the hub (only choices that differ from hub.toml are saved as picked).
        let palette = ui.global::<Palette>();
        if palette.get_chosen() == before.accent {
            palette.set_chosen(now.accent);
        }
        if hub.get_fullscreen() == before.fullscreen {
            hub.set_fullscreen(now.fullscreen);
        }

        let changed_devices = {
            let config = self.config.borrow();
            (old.outputs != config.outputs || old.inputs != config.inputs).then(|| device_choices(&config))
        };
        if let Some(choices) = changed_devices {
            let _ = self.audio.send(AudioCommand::SetChoices(choices));
        }
        let moved = old.window != self.config.borrow().window;
        if moved || ui.window().is_fullscreen() != hub.get_fullscreen() {
            window::place(ui.window(), &self.placement(hub.get_fullscreen()));
        }
        if old.hotkeys != self.config.borrow().hotkeys {
            self.start_hotkeys();
        }
        if old.weather != self.config.borrow().weather && self.weather_place.borrow().is_none() {
            self.refresh_weather();
        }
        {
            let config = self.config.borrow();
            if old.calendar != config.calendar {
                self.calendar_due.set(true);
            }
            if old.notifications != config.notifications {
                let _ = self.notifications.0.send(notifications::Command::Apps(config.notifications.apps.clone()));
            }
            if old.discord != config.discord {
                let _ = self.discord.0.send(discord::Command::Config(config.discord.clone()));
            }
            if old.obs != config.obs {
                let _ = self.obs.0.send(obs::Command::Config(config.obs()));
            }
        }
        self.apply_shortcuts(ui);
    }

    /// Changes hub.toml's shortcuts with `edit`, which returns what to say under the
    /// Shortcuts title, then shows the new tiles. Returns the error, if any.
    fn edit_shortcuts(&self, edit: impl FnOnce(&Path) -> Result<String, String>) -> Result<(), String> {
        let Some(ui) = self.ui.upgrade() else { return Ok(()) };
        let result = match self.config_path.as_deref() {
            None => Err("there's no hub.toml to change".to_string()),
            Some(path) => edit(path).and_then(|message| Ok((message, config::read(path)?))).map(|(message, config)| {
                self.config_stamp.set(modified(path));
                self.apply_config(&ui, config);
                message
            }),
        };
        let message = match &result {
            Ok(message) => message.clone(),
            Err(err) => {
                eprintln!("config: could not change the tiles: {err}");
                format!("Could not change the tiles: {err}")
            }
        };
        ui.global::<Hub>().set_last_action(message.into());
        result.map(drop)
    }

    /// Files dragged from Explorer: highlight the hub, and add a tile for each one dropped.
    fn file_event(&self, event: window::FileEvent) {
        let Some(ui) = self.ui.upgrade() else { return };
        let hub = ui.global::<Hub>();
        match event {
            window::FileEvent::Hovering => hub.set_file_hovering(true),
            window::FileEvent::Left => hub.set_file_hovering(false),
            window::FileEvent::Dropped(target) => {
                hub.set_file_hovering(false);
                let _ = self.edit_shortcuts(|path| {
                    let label = config::add_shortcut_for(path, &target)?;
                    Ok(format!("Added {label}"))
                });
                if hub.get_show_shortcuts() {
                    hub.set_slide(2);
                }
            }
            window::FileEvent::Link { url, title } => {
                hub.set_file_hovering(false);
                let _ = self.edit_shortcuts(|path| {
                    let label = config::add_link_shortcut(path, &url, title.as_deref())?;
                    Ok(format!("Added {label}"))
                });
                if hub.get_show_shortcuts() {
                    hub.set_slide(2);
                }
            }
        }
    }

    /// Opens a folder tile: its tiles show in a panel over the page.
    fn open_folder(&self, ui: &AppWindow, folder: &str, label: &str) {
        let hub = ui.global::<Hub>();
        let all = hub.get_all_shortcuts();
        let config = self.config.borrow();
        let inside: Vec<Shortcut> = (0..all.row_count())
            .filter_map(|row| all.row_data(row))
            .filter(|tile| {
                config.shortcuts[tile.index as usize]
                    .in_folder()
                    .is_some_and(|f| f.eq_ignore_ascii_case(folder.trim()))
            })
            .collect();
        hub.set_folder_title(label.into());
        hub.set_folder_tiles(ModelRc::from(Rc::new(VecModel::from(inside))));
        hub.set_folder_open(true);
    }

    /// Fills in the screen choices on the Customize hub screen.
    fn refresh_screens(&self, ui: &AppWindow) {
        let choices: Vec<ScreenChoice> = window::screens(ui.window())
            .iter()
            .enumerate()
            .map(|(i, screen)| ScreenChoice {
                label: format!("Screen {}", i + 1).into(),
                active: screen.current,
            })
            .collect();
        ui.global::<Hub>().set_screens(ModelRc::from(Rc::new(VecModel::from(choices))));
    }
}

/// Changes the tile at `index` in hub.toml, wherever it shows (the current page and the
/// Customize hub list), unless the tiles were reloaded since `generation`.
fn update_tile(ui: &AppWindow, tiles: &AtomicU64, generation: u64, index: usize, change: impl Fn(&mut Shortcut)) {
    if tiles.load(Ordering::SeqCst) != generation {
        return;
    }
    let hub = ui.global::<Hub>();
    for model in [hub.get_shortcuts(), hub.get_all_shortcuts(), hub.get_folder_tiles()] {
        for row in 0..model.row_count() {
            if let Some(mut tile) = model.row_data(row).filter(|t| t.index == index as i32) {
                change(&mut tile);
                model.set_row_data(row, tile);
            }
        }
    }
}

fn device_choices(config: &config::Config) -> audio::DeviceChoices {
    audio::DeviceChoices {
        outputs: config.outputs.clone(),
        inputs: config.inputs.clone(),
    }
}

fn connect_shortcuts(ui: &AppWindow, app: &Rc<App>) {
    let hub = ui.global::<Hub>();
    // Clears a "tap again to confirm" that wasn't followed up.
    let disarm = Timer::default();
    hub.on_launch_shortcut({
        let app = app.clone();
        move |index| {
            let Some(shortcut) = app.config.borrow().shortcuts.get(index as usize).cloned() else {
                return;
            };
            let Some(ui) = app.ui.upgrade() else { return };
            let hub = ui.global::<Hub>();
            if shortcut.needs_confirm() && hub.get_armed_shortcut() != index {
                hub.set_armed_shortcut(index);
                hub.set_last_action(format!("Tap {} again to confirm", shortcut.label).into());
                let weak = app.ui.clone();
                disarm.start(TimerMode::SingleShot, Duration::from_secs(4), move || {
                    if let Some(ui) = weak.upgrade() {
                        ui.global::<Hub>().set_armed_shortcut(-1);
                    }
                });
                return;
            }
            hub.set_armed_shortcut(-1);
            disarm.stop();

            if let Some(folder) = &shortcut.folder {
                app.open_folder(&ui, folder, &shortcut.label);
                return;
            }
            if shortcut.work().is_some() {
                hub.set_last_action(format!("Running {}…", shortcut.label).into());
                app.run_work(index as usize, true);
                return;
            }
            // Wi-Fi and Bluetooth take a moment to switch; don't hold up the screen.
            if matches!(shortcut.toggle(), "wifi" | "bluetooth" | "night-light" | "dnd") {
                let (weak, label, name) = (app.ui.clone(), shortcut.label.clone(), shortcut.toggle());
                std::thread::spawn(move || {
                    let result = toggles::toggle(name);
                    let _ = weak.upgrade_in_event_loop(move |ui| {
                        let message = match result {
                            Ok(()) => format!("Last: {label} · {}", chrono::Local::now().format("%H:%M")),
                            Err(err) => {
                                eprintln!("shortcut \"{label}\": {err}");
                                format!("Could not run {label}: {err}")
                            }
                        };
                        ui.global::<Hub>().set_last_action(message.into());
                    });
                });
                return;
            }
            let result = match shortcut.toggle() {
                "mic" => app.audio.send(AudioCommand::ToggleMicMute).map_err(|e| e.to_string()),
                "mute" => app.audio.send(AudioCommand::ToggleMute).map_err(|e| e.to_string()),
                _ => shortcut.launch(),
            };
            let time = chrono::Local::now().format("%H:%M");
            let message = match result {
                Ok(()) => format!("Last: {} · {time}", shortcut.label),
                Err(err) => {
                    eprintln!("shortcut \"{}\": {err}", shortcut.label);
                    format!("Could not run {}: {err}", shortcut.label)
                }
            };
            hub.set_last_action(message.into());
        }
    });
    hub.on_hold_shortcut({
        let app = app.clone();
        move |index| {
            let Some(shortcut) = app.config.borrow().shortcuts.get(index as usize).cloned() else { return };
            let Some(step) = shortcut.hold.clone() else { return };
            let (weak, audio) = (app.ui.clone(), app.audio.clone());
            if let Some(ui) = app.ui.upgrade() {
                ui.global::<Hub>().set_last_action(format!("Holding {}…", shortcut.label).into());
            }
            std::thread::spawn(move || {
                let result = commands::run_work(&config::Work::Steps(vec![step]), &audio);
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    let message = match result {
                        Ok(_) => format!("Last: {} (held) · {}", shortcut.label, chrono::Local::now().format("%H:%M")),
                        Err(err) => format!("Could not run {} (held): {err}", shortcut.label),
                    };
                    ui.global::<Hub>().set_last_action(message.into());
                });
            });
        }
    });
    hub.on_edit_config({
        let app = app.clone();
        move || match &app.config_path {
            Some(path) => actions::edit_file(path),
            None => eprintln!("config: no hub.toml to edit"),
        }
    });
    hub.on_move_shortcut({
        let app = app.clone();
        move |from, to| {
            // The UI gives places among the tiles shown, which on a shortcut page are only
            // some of them; move between the same tiles' places in hub.toml.
            let Some(ui) = app.ui.upgrade() else { return };
            let shown = ui.global::<Hub>().get_shortcuts();
            let place = |i: i32| shown.row_data(i.max(0) as usize).map(|tile| tile.index.max(0) as usize);
            let (Some(from), Some(to)) = (place(from), place(to)) else { return };
            let _ = app.edit_shortcuts(|path| {
                config::move_shortcut(path, from, to)?;
                Ok(format!("Moved to place {}", to + 1))
            });
        }
    });
    hub.on_choose_shortcut_page({
        let app = app.clone();
        move |page| {
            let Some(ui) = app.ui.upgrade() else { return };
            let name = app.config.borrow().page_names().get(page.max(0) as usize).cloned();
            app.show_shortcut_page(&ui, name, true);
        }
    });
    hub.on_timer_finished(actions::chime);
    hub.on_delete_shortcut({
        let app = app.clone();
        move |index| {
            let _ = app.edit_shortcuts(|path| {
                let label = config::remove_shortcut(path, index.max(0) as usize)?;
                Ok(format!("Deleted {label}"))
            });
        }
    });
}

fn connect_media(ui: &AppWindow, media: Sender<MediaCommand>) {
    let hub = ui.global::<Hub>();
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
    hub.on_media_seek({
        let media = media.clone();
        move |fraction| {
            let _ = media.send(MediaCommand::Seek(fraction));
        }
    });
    hub.on_choose_player(move |index| {
        let _ = media.send(MediaCommand::ChoosePlayer(index));
    });
}

fn connect_audio(ui: &AppWindow, audio: Sender<AudioCommand>) {
    let hub = ui.global::<Hub>();
    let audio_command = |command: AudioCommand| {
        let audio = audio.clone();
        move || {
            let _ = audio.send(command.clone());
        }
    };
    hub.on_toggle_mute(audio_command(AudioCommand::ToggleMute));
    hub.on_toggle_mic_mute(audio_command(AudioCommand::ToggleMicMute));
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
    hub.on_select_input({
        let audio = audio.clone();
        move |index| {
            let _ = audio.send(AudioCommand::SelectInput(index.max(0) as usize));
        }
    });
    hub.on_set_app_volume({
        let audio = audio.clone();
        move |index, level| {
            let _ = audio.send(AudioCommand::SetAppVolume(index.max(0) as usize, level));
        }
    });
    hub.on_toggle_app_mute(move |index| {
        let _ = audio.send(AudioCommand::ToggleAppMute(index.max(0) as usize));
    });
}

fn connect_settings(ui: &AppWindow, app: &Rc<App>) {
    let hub = ui.global::<Hub>();

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
    hub.on_text_edit(|text, key, mode| edit_text(&text, &key, &mode).into());
    hub.on_refresh_weather_now({
        let app = app.clone();
        move || app.weather_due.set(true)
    });
    hub.on_keyboard_focus({
        let weak = ui.as_weak();
        move |take| {
            if let Some(ui) = weak.upgrade() {
                actions::take_keyboard_focus(ui.window(), take);
            }
        }
    });
    hub.on_set_weather_place({
        let app = app.clone();
        move |place| {
            let Some(ui) = app.ui.upgrade() else { return };
            let place = place.trim().to_string();
            ui.global::<Hub>().set_weather_place(place.as_str().into());
            *app.weather_place.borrow_mut() = (!place.is_empty()).then_some(place);
            app.refresh_weather();
            app.save_settings(&ui);
        }
    });
    hub.on_hex_color(|digits| config::parse_color(&digits).unwrap_or_default());

    // Settings sheet and Customize hub screen
    hub.on_settings_changed({
        let app = app.clone();
        move || {
            let Some(ui) = app.ui.upgrade() else { return };
            apply_theme(&ui);
            apply_language(settings::LANGUAGES[ui.global::<Hub>().get_language().clamp(0, settings::LANGUAGES.len() as i32 - 1) as usize]);
            let fullscreen = ui.global::<Hub>().get_fullscreen();
            if ui.window().is_fullscreen() != fullscreen {
                window::place(ui.window(), &app.placement(fullscreen));
            }
            app.save_settings(&ui);
        }
    });
    hub.on_reset_settings({
        let app = app.clone();
        move || {
            let Some(ui) = app.ui.upgrade() else { return };
            let reset = settings::Settings::default();
            app.geometry.set(None);
            app.screen.set(None);
            *app.weather_place.borrow_mut() = None;
            app.refresh_weather();
            apply_settings(&ui, &reset, app.defaults());
            window::place(ui.window(), &app.placement(app.defaults().fullscreen));
            settings::save(&app.settings_path, &reset);
        }
    });

    hub.set_start_with_windows(actions::start_with_windows());
    hub.on_set_start_with_windows({
        let weak = ui.as_weak();
        move |on| {
            let Some(ui) = weak.upgrade() else { return };
            let hub = ui.global::<Hub>();
            if let Err(err) = actions::set_start_with_windows(on) {
                eprintln!("start with Windows: {err}");
                hub.set_last_action(format!("Could not change Start with Windows: {err}").into());
            }
            // Show what Windows actually has, in case the change didn't take.
            hub.set_start_with_windows(actions::start_with_windows());
        }
    });
    hub.on_open_update_page({
        let app = app.clone();
        move || {
            let page = match app.update.lock().ok().and_then(|u| u.clone()) {
                Some(Ok(Some((_, page)))) => page,
                _ => "https://github.com/sudpmishra/PC-Hub/releases".to_string(),
            };
            if let Err(err) = config::spawn(&page, &[]) {
                eprintln!("updates: could not open {page}: {err}");
            }
        }
    });
    hub.on_close_hub(|| {
        let _ = slint::quit_event_loop();
    });
}

fn connect_window(ui: &AppWindow, app: &Rc<App>) {
    let hub = ui.global::<Hub>();

    // Out of full screen, the top bar moves the window and the corner grip resizes it.
    // The UI reports how far the pointer moved, in logical pixels.
    hub.on_move_window({
        let weak = ui.as_weak();
        move |dx, dy| {
            let Some(ui) = weak.upgrade() else { return };
            let window = ui.window();
            let scale = window.scale_factor();
            let position = window.position();
            window::move_snapped(
                window,
                PhysicalPosition::new(
                    position.x + (dx * scale).round() as i32,
                    position.y + (dy * scale).round() as i32,
                ),
            );
        }
    });
    hub.on_resize_window({
        let weak = ui.as_weak();
        move |dx, dy| {
            let Some(ui) = weak.upgrade() else { return };
            let window = ui.window();
            let scale = window.scale_factor();
            let size = window.size();
            let [min_width, min_height] = settings::MIN_WINDOW;
            window.set_size(PhysicalSize::new(
                (size.width as i32 + (dx * scale).round() as i32).max(min_width) as u32,
                (size.height as i32 + (dy * scale).round() as i32).max(min_height) as u32,
            ));
        }
    });
    hub.on_window_placed({
        let app = app.clone();
        move || {
            let Some(ui) = app.ui.upgrade() else { return };
            if ui.window().is_fullscreen() {
                return;
            }
            app.geometry.set(Some(window::geometry(ui.window())));
            app.save_settings(&ui);
        }
    });

    hub.on_refresh_screens({
        let app = app.clone();
        move || {
            if let Some(ui) = app.ui.upgrade() {
                app.refresh_screens(&ui);
            }
        }
    });
    hub.on_choose_screen({
        let app = app.clone();
        move |index| {
            let Some(ui) = app.ui.upgrade() else { return };
            let screens = window::screens(ui.window());
            let Some(screen) = screens.get(index.max(0) as usize) else { return };
            app.screen.set(Some(screen.corner));
            if ui.global::<Hub>().get_fullscreen() {
                window::place(ui.window(), &app.placement(true));
            } else {
                window::move_to_screen(ui.window(), screen.corner);
                app.geometry.set(Some(window::geometry(ui.window())));
            }
            app.save_settings(&ui);
            app.refresh_screens(&ui);
        }
    });
}

/// Built-in actions the tile editor offers, with their names on the hub.
const EDITOR_ACTIONS: [(&str, &str); 14] = [
    ("lock", "Lock"),
    ("sleep", "Sleep"),
    ("hibernate", "Hibernate"),
    ("shutdown", "Shut down"),
    ("restart", "Restart"),
    ("signout", "Sign out"),
    ("screen-off", "Screens off"),
    ("mic-mute", "Mute mic"),
    ("mute", "Mute sound"),
    ("dark-mode", "Dark mode"),
    ("empty-recycle-bin", "Empty bin"),
    ("power-saver", "Power saver"),
    ("balanced", "Balanced"),
    ("high-performance", "Performance"),
];

/// The tile editor in Customize hub: loads a tile from hub.toml and saves it back.
fn connect_tile_editor(ui: &AppWindow, app: &Rc<App>) {
    let editor = ui.global::<TileEditor>();
    let icons: Vec<IconChoice> = icons::NAMES
        .iter()
        .map(|name| IconChoice {
            name: (*name).into(),
            glyph: icons::glyph(Some(name)).into(),
        })
        .collect();
    editor.set_icons(ModelRc::from(Rc::new(VecModel::from(icons))));
    let actions: Vec<Choice> = EDITOR_ACTIONS
        .iter()
        .map(|(name, label)| Choice {
            name: (*name).into(),
            label: (*label).into(),
        })
        .collect();
    editor.set_actions(ModelRc::from(Rc::new(VecModel::from(actions))));

    editor.on_begin({
        let app = app.clone();
        move |index| {
            let Some(ui) = app.ui.upgrade() else { return };
            let editor = ui.global::<TileEditor>();
            let tile = usize::try_from(index).ok().and_then(|i| app.config.borrow().shortcuts.get(i).cloned());
            let does = tile.as_ref().map_or(Some(config::TileAction::Open(String::new())), config::tile_action);
            editor.set_index(if tile.is_some() { index } else { -1 });
            editor.set_label(tile.as_ref().map(|t| t.label.as_str()).unwrap_or_default().into());
            editor.set_icon(tile.as_ref().and_then(|t| t.icon.as_deref()).unwrap_or_default().into());
            // A new tile goes on the page being shown.
            let page = match &tile {
                Some(tile) => tile.page.clone(),
                None => app.shortcut_page.borrow().clone().filter(|_| !app.config.borrow().page_names().is_empty()),
            };
            editor.set_page(page.unwrap_or_default().into());
            let (mut ctrl, mut shift, mut alt, mut win, mut key) = (false, false, false, false, String::new());
            let (mut action, mut target) = (String::new(), String::new());
            let kind = match &does {
                Some(config::TileAction::Keys(keys)) => {
                    for part in keys.split('+').map(str::trim) {
                        match part.to_lowercase().as_str() {
                            "ctrl" | "control" => ctrl = true,
                            "shift" => shift = true,
                            "alt" => alt = true,
                            "win" | "windows" | "super" => win = true,
                            _ => key = part.to_string(),
                        }
                    }
                    "keys"
                }
                Some(config::TileAction::Action(name)) => {
                    action = name.clone();
                    "action"
                }
                Some(config::TileAction::Open(run)) => {
                    target = run.clone();
                    "open"
                }
                None => "other",
            };
            editor.set_kind(kind.into());
            editor.set_ctrl(ctrl);
            editor.set_shift(shift);
            editor.set_alt(alt);
            editor.set_win(win);
            editor.set_key(key.into());
            editor.set_action(action.into());
            editor.set_target(target.into());
            editor.set_error("".into());
            editor.set_open(true);
        }
    });

    editor.on_save({
        let app = app.clone();
        move || {
            let Some(ui) = app.ui.upgrade() else { return };
            let editor = ui.global::<TileEditor>();
            let text = |value: slint::SharedString| Some(value.to_string()).filter(|v| !v.trim().is_empty());
            let does = match editor.get_kind().as_str() {
                "keys" => {
                    let mut keys: Vec<&str> = Vec::new();
                    for (on, name) in [
                        (editor.get_ctrl(), "Ctrl"),
                        (editor.get_shift(), "Shift"),
                        (editor.get_alt(), "Alt"),
                        (editor.get_win(), "Win"),
                    ] {
                        if on {
                            keys.push(name);
                        }
                    }
                    let key = editor.get_key();
                    if key.is_empty() {
                        editor.set_error("Pick a key to press: tap Key…".into());
                        return;
                    }
                    keys.push(key.as_str());
                    Some(config::TileAction::Keys(keys.join("+")))
                }
                "action" => Some(config::TileAction::Action(editor.get_action().to_string())),
                "open" => Some(config::TileAction::Open(editor.get_target().to_string())),
                _ => None,
            };
            let edit = config::TileEdit {
                label: editor.get_label().to_string(),
                icon: text(editor.get_icon()),
                page: text(editor.get_page()),
                does,
            };
            let index = usize::try_from(editor.get_index()).ok();
            let saved = app.edit_shortcuts(|path| {
                config::save_tile(path, index, &edit)?;
                Ok(format!("Saved {}", edit.label.trim()))
            });
            match saved {
                Ok(()) => editor.set_open(false),
                Err(err) => editor.set_error(format!("Couldn't save: {err}").into()),
            }
        }
    });
}

/// Backups of hub.toml and the hub's settings, in Customize hub > Display & power.
fn connect_backups(ui: &AppWindow, app: &Rc<App>) {
    let hub = ui.global::<Hub>();
    hub.on_refresh_backups({
        let app = app.clone();
        move || {
            if let Some(ui) = app.ui.upgrade() {
                app.refresh_backups(&ui);
            }
        }
    });
    hub.on_back_up({
        let app = app.clone();
        move || {
            let Some(ui) = app.ui.upgrade() else { return };
            let status = match backup::create(&app.config_dir()) {
                Ok(name) => format!("Backed up {}", backup::describe(&name)),
                Err(err) => format!("Couldn't back up: {err}"),
            };
            ui.global::<Hub>().set_backup_status(status.into());
            app.refresh_backups(&ui);
        }
    });
    hub.on_restore_backup({
        let app = app.clone();
        move |name| {
            let Some(ui) = app.ui.upgrade() else { return };
            let status = match backup::restore(&app.config_dir(), &name) {
                Ok(()) => {
                    app.reload_everything(&ui);
                    format!("Restored the backup from {}", backup::describe(&name))
                }
                Err(err) => format!("Couldn't restore: {err}"),
            };
            ui.global::<Hub>().set_backup_status(status.into());
            app.refresh_backups(&ui);
        }
    });
}

/// The Widgets page's buttons: notifications, clipboard, Discord and OBS.
fn connect_widgets(ui: &AppWindow, app: &Rc<App>) {
    let hub = ui.global::<Hub>();
    hub.on_dismiss_notice({
        let app = app.clone();
        move |id| {
            let _ = app.notifications.0.send(notifications::Command::Dismiss(id.max(0) as u32));
        }
    });
    hub.on_dismiss_all_notices({
        let app = app.clone();
        move || {
            let _ = app.notifications.0.send(notifications::Command::DismissAll);
        }
    });
    hub.on_paste_clip({
        let app = app.clone();
        move |index| {
            let result = app.clipboard.borrow_mut().paste(index.max(0) as usize);
            let Some(ui) = app.ui.upgrade() else { return };
            if let Err(err) = result {
                ui.global::<Hub>().set_clips_status(format!("Couldn't paste: {err}").into());
            }
            show_clips(&ui, &app.clipboard.borrow());
        }
    });
    hub.on_clear_clips({
        let app = app.clone();
        move || {
            app.clipboard.borrow_mut().clear();
            if let Some(ui) = app.ui.upgrade() {
                show_clips(&ui, &app.clipboard.borrow());
            }
        }
    });
    hub.on_discord_mute({
        let app = app.clone();
        move || {
            let _ = app.discord.0.send(discord::Command::ToggleMute);
        }
    });
    hub.on_discord_deafen({
        let app = app.clone();
        move || {
            let _ = app.discord.0.send(discord::Command::ToggleDeaf);
        }
    });
    hub.on_obs_scene({
        let app = app.clone();
        move |name| {
            let _ = app.obs.0.send(obs::Command::Scene(name.to_string()));
        }
    });
    hub.on_obs_record({
        let app = app.clone();
        move || {
            let _ = app.obs.0.send(obs::Command::ToggleRecord);
        }
    });
    hub.on_obs_stream({
        let app = app.clone();
        move || {
            let _ = app.obs.0.send(obs::Command::ToggleStream);
        }
    });
    hub.on_refresh_calendar({
        let app = app.clone();
        move || app.calendar_due.set(true)
    });
    hub.on_spotify_like({
        let weak = ui.as_weak();
        move || {
            let message = match actions::press_in_app("spotify", "Alt+Shift+B") {
                Ok(()) => "Liked in Spotify (or unliked, if it was)".to_string(),
                Err(err) => format!("Couldn't like: {err}"),
            };
            if let Some(ui) = weak.upgrade() {
                ui.global::<Hub>().set_last_action(message.into());
            }
        }
    });
}

/// The clipboard card's entries.
fn show_clips(ui: &AppWindow, history: &clipboard::History) {
    let clips: Vec<slint::SharedString> = history.items.iter().map(|t| clipboard::preview(t).into()).collect();
    ui.global::<Hub>().set_clips(ModelRc::from(Rc::new(VecModel::from(clips))));
}

/// Profiles in Customize hub > Display & power.
fn connect_profiles(ui: &AppWindow, app: &Rc<App>) {
    let hub = ui.global::<Hub>();
    let refresh = {
        let app = app.clone();
        move |status: Option<String>| {
            let Some(ui) = app.ui.upgrade() else { return };
            let hub = ui.global::<Hub>();
            let dir = app.config_dir();
            let names: Vec<slint::SharedString> = profiles::list(&dir).iter().map(|n| n.as_str().into()).collect();
            hub.set_profiles(ModelRc::from(Rc::new(VecModel::from(names))));
            hub.set_active_profile(profiles::active(&dir).unwrap_or_default().into());
            if let Some(status) = status {
                hub.set_profile_status(status.into());
            }
        }
    };
    hub.on_refresh_profiles({
        let refresh = refresh.clone();
        move || refresh(None)
    });
    hub.on_save_profile({
        let (app, refresh) = (app.clone(), refresh.clone());
        move |name| {
            let status = match profiles::save(&app.config_dir(), &name) {
                Ok(name) => format!("Saved the current setup as {name}"),
                Err(err) => format!("Couldn't save: {err}"),
            };
            refresh(Some(status));
        }
    });
    hub.on_switch_profile({
        let (app, refresh) = (app.clone(), refresh.clone());
        move |name| {
            let Some(ui) = app.ui.upgrade() else { return };
            let status = match profiles::switch(&app.config_dir(), &name) {
                Ok(()) => {
                    app.reload_everything(&ui);
                    format!("Switched to {name}")
                }
                Err(err) => format!("Couldn't switch: {err}"),
            };
            refresh(Some(status));
        }
    });
    hub.on_delete_profile({
        let (app, refresh) = (app.clone(), refresh.clone());
        move |name| {
            let status = match profiles::delete(&app.config_dir(), &name) {
                Ok(()) => format!("Deleted {name}"),
                Err(err) => format!("Couldn't delete: {err}"),
            };
            refresh(Some(status));
        }
    });
}

/// Picks the interface language: a code from settings::LANGUAGES, "system" following Windows.
fn apply_language(code: &str) {
    let code = if code == "system" { actions::windows_language() } else { code.to_string() };
    let code = if settings::LANGUAGES.contains(&code.as_str()) { code } else { "en".into() };
    if let Err(err) = slint::select_bundled_translation(if code == "en" { "" } else { &code }) {
        eprintln!("language: {err}");
    }
}

/// The clock ticks every second; hardware readings follow the refresh interval setting.
/// The first readings arrive on the first tick: load needs two samples a moment apart.
fn start_ticking(ui: &AppWindow, app: &Rc<App>) -> Timer {
    let usage_path = network::DailyUsage::path_beside(app.config_path.as_deref());
    let monitor = Rc::new(RefCell::new(Monitor::new(usage_path)));
    let hub = ui.global::<Hub>();
    hub.set_system_summary(monitor.borrow().summary().into());
    hub.on_end_app({
        let (weak, monitor) = (ui.as_weak(), monitor.clone());
        move |name| {
            let ended = monitor.borrow_mut().stats.end_app(&name);
            eprintln!("processes: ended {ended} of {name}");
            if let Some(ui) = weak.upgrade() {
                monitor.borrow_mut().refresh_top_apps(&ui);
            }
        }
    });
    // Customize hub > Hardware monitor: pick HWiNFO readings for cards of their own.
    hub.on_refresh_hwinfo_readings({
        let (weak, monitor) = (ui.as_weak(), monitor.clone());
        move || {
            let Some(ui) = weak.upgrade() else { return };
            let fahrenheit = ui.global::<Hub>().get_fahrenheit();
            let rows: Vec<ReadingChoice> = monitor
                .borrow()
                .snapshot
                .as_ref()
                .map(|snapshot| {
                    snapshot
                        .readings
                        .iter()
                        .map(|r| {
                            let (value, unit) = hwinfo::display(r, fahrenheit);
                            ReadingChoice {
                                sensor: r.sensor.as_str().into(),
                                label: r.label.as_str().into(),
                                value: format!("{value} {unit}").trim().into(),
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            ui.global::<Hub>().set_hwinfo_readings(ModelRc::from(Rc::new(VecModel::from(rows))));
        }
    });
    hub.on_add_sensor_card({
        let app = app.clone();
        move |sensor, label| {
            let card = config::SensorCard {
                label: label.to_string(),
                sensor: Some(sensor.to_string()).filter(|s| !s.is_empty()),
                reading: label.to_string(),
                max: None,
            };
            let _ = app.edit_shortcuts(|path| {
                config::add_sensor_card(path, &card)?;
                Ok(format!("Added a card for {}", card.label))
            });
        }
    });
    hub.on_remove_sensor_card({
        let app = app.clone();
        move |index| {
            let _ = app.edit_shortcuts(|path| {
                config::remove_sensor_card(path, index.max(0) as usize)?;
                Ok("Removed the card".to_string())
            });
        }
    });
    // The page that was showing before one turned on by itself for a running app.
    let mut auto_page: Option<String> = None;
    let mut page_before_auto: Option<String> = None;
    update_clock(ui);
    let tick = Timer::default();
    let app = app.clone();
    let mut seconds = 0u64;
    tick.start(TimerMode::Repeated, Duration::from_secs(1), move || {
        let Some(ui) = app.ui.upgrade() else { return };
        seconds += 1;
        update_clock(&ui);
        let hub = ui.global::<Hub>();
        hub.set_pc_locked(hub.get_dark_when_locked() && actions::session_locked());
        let every = hub.get_refresh_seconds().max(1) as u64;
        if seconds.is_multiple_of(every) {
            let (cpu_fan, drives, cards) = {
                let config = app.config.borrow();
                (config.sensors.cpu_fan.clone(), config.drives.clone(), config.cards.clone())
            };
            monitor.borrow_mut().refresh(&ui, cpu_fan.as_deref(), &drives, &cards);
        }
        if hub.get_processes_open() && seconds.is_multiple_of(2) {
            monitor.borrow_mut().refresh_top_apps(&ui);
        }
        if seconds.is_multiple_of(30) || seconds == 1 {
            show_battery(&ui);
        }
        update_weather(&app, &ui, seconds);

        // A shortcut page whose app is running turns on by itself; when that app
        // closes, the page from before comes back.
        if seconds.is_multiple_of(5) {
            let watched: Vec<config::PageConfig> =
                app.config.borrow().pages.iter().filter(|p| !p.when_running.is_empty()).cloned().collect();
            if !watched.is_empty() {
                let running = monitor.borrow_mut().stats.running_programs();
                let matched = watched
                    .iter()
                    .find(|page| {
                        page.when_running
                            .iter()
                            .any(|program| running.contains(&stats::program_name(program).to_lowercase()))
                    })
                    .map(|page| page.name.clone());
                if matched != auto_page {
                    match &matched {
                        Some(name) => {
                            if auto_page.is_none() {
                                page_before_auto = app.shortcut_page.borrow().clone();
                            }
                            app.show_shortcut_page(&ui, Some(name.clone()), false);
                        }
                        None => app.show_shortcut_page(&ui, page_before_auto.take(), false),
                    }
                    auto_page = matched;
                }
            }
        }
        // Command tiles that refresh by themselves.
        let due: Vec<usize> = app
            .config
            .borrow()
            .shortcuts
            .iter()
            .enumerate()
            .filter(|(_, s)| s.refresh_secs().is_some_and(|n| seconds.is_multiple_of(n)))
            .map(|(i, _)| i)
            .collect();
        for index in due {
            app.run_work(index, false);
        }
        if seconds.is_multiple_of(2) {
            // Following Windows' theme: notice when it changes.
            if hub.get_theme() == 2 {
                apply_theme(&ui);
            }
            // Lights dark-mode tiles.
            hub.set_windows_dark(!actions::windows_light_theme());
        }
        update_latency(&app, &ui, seconds);
        update_check(&app, &ui, seconds);
        update_widgets(&app, &ui, seconds);
        update_extras(&app, &ui, seconds);
    });
    tick
}

/// Lights toggle tiles, follows the album art accent and the backlight, runs the
/// microphone meter only while it shows, and looks up wireless devices' batteries.
fn update_extras(app: &App, ui: &AppWindow, seconds: u64) {
    let hub = ui.global::<Hub>();
    if let Ok(states) = app.toggles.lock() {
        hub.set_wifi_on(states.wifi.unwrap_or(false));
        hub.set_bluetooth_on(states.bluetooth.unwrap_or(false));
        hub.set_night_light_on(states.night_light.unwrap_or(false));
        hub.set_dnd_on(states.dnd.unwrap_or(false));
    }
    let palette = ui.global::<Palette>();
    palette.set_use_art(hub.get_art_accent() && hub.get_media_playing() && hub.get_has_art() && palette.get_has_art_color());

    let idle = ui.global::<Activity>().get_idle_seconds();
    let dimmed = hub.get_idle_dim() && idle >= hub.get_idle_minutes() * 60;
    let dark = hub.get_dark_when_locked() && hub.get_pc_locked();
    app.mic_meter.store(
        hub.get_slide() == 0 && hub.get_show_music() && hub.get_show_inputs() && hub.get_show_mic_meter() && !dimmed && !dark,
        Ordering::Relaxed,
    );

    // The monitor's own backlight, when chosen: dimmed and locked turn it right down.
    if hub.get_hardware_brightness() {
        if let Some(window) = actions::native_handle(ui.window()) {
            let percent = if dark {
                0
            } else if dimmed {
                5
            } else {
                hub.get_brightness().max(0) as u32
            };
            app.backlight.set(window, percent);
        }
        let (active, status) = match app.backlight.works() {
            None => (false, "Trying the monitor's backlight…".to_string()),
            Some(Ok(())) => (true, "Changes the monitor's own backlight".to_string()),
            Some(Err(err)) => (false, format!("Not available: {err}; darkening the picture instead")),
        };
        hub.set_backlight_active(active);
        hub.set_backlight_status(status.into());
    } else {
        hub.set_backlight_active(false);
        hub.set_backlight_status("Off: darkens the picture. On: changes the monitor's own backlight (DDC/CI)".into());
        app.backlight.forget();
    }

    // Wireless devices' batteries, looked up every minute.
    if seconds % 60 == 2 {
        let latest = app.batteries.clone();
        std::thread::spawn(move || {
            let found = devices::batteries();
            if let Ok(mut latest) = latest.lock() {
                *latest = found;
            }
        });
    }
    if seconds.is_multiple_of(5) {
        let rows: Vec<DeviceBatteryRow> = app
            .batteries
            .lock()
            .map(|b| {
                b.iter()
                    .map(|d| DeviceBatteryRow {
                        name: d.name.as_str().into(),
                        percent: f32::from(d.percent) / 100.0,
                        text: format!("{}%", d.percent).into(),
                        low: d.percent <= 20,
                    })
                    .collect()
            })
            .unwrap_or_default();
        set_rows(hub.get_device_batteries(), rows, |m| hub.set_device_batteries(m));
    }
}

/// Replaces a model's rows only when they changed, so lists don't jump while touched.
fn set_rows<T: Clone + PartialEq + 'static>(current: ModelRc<T>, rows: Vec<T>, set: impl FnOnce(ModelRc<T>)) {
    let same = current.row_count() == rows.len() && rows.iter().enumerate().all(|(i, r)| current.row_data(i).as_ref() == Some(r));
    if !same {
        set(ModelRc::from(Rc::new(VecModel::from(rows))));
    }
}

/// The Widgets page: calendar, notifications, clipboard, Discord and OBS.
fn update_widgets(app: &App, ui: &AppWindow, seconds: u64) {
    let hub = ui.global::<Hub>();

    // Calendar: downloaded every 10 minutes; past events drop off as they end.
    let urls = app.config.borrow().calendar.urls.clone();
    if urls.is_empty() {
        hub.set_calendar_status("Not set up: add calendar links under [calendar] in hub.toml".into());
        set_rows(hub.get_calendar_events(), Vec::new(), |m| hub.set_calendar_events(m));
    } else {
        if app.calendar_due.replace(false) || seconds.is_multiple_of(600) {
            let latest = app.calendar.clone();
            std::thread::spawn(move || {
                let found = calendar::upcoming(&urls, chrono::Local::now().naive_local());
                for err in &found.1 {
                    eprintln!("calendar: {err}");
                }
                if let Ok(mut latest) = latest.lock() {
                    *latest = Some(found);
                }
            });
        }
        if seconds.is_multiple_of(5) || seconds == 1 {
            let now = chrono::Local::now().naive_local();
            let today = now.date();
            let latest = app.calendar.lock().ok().and_then(|c| c.clone());
            let (rows, status) = match latest {
                None => (Vec::new(), "Looking at your calendar…".to_string()),
                Some((events, errors)) => {
                    let rows: Vec<CalendarEvent> = events
                        .iter()
                        .filter(|e| e.end > now || (e.all_day && e.start.date() >= today))
                        .take(12)
                        .map(|e| CalendarEvent {
                            day: if e.start.date() <= today { "Today".into() } else { "Tomorrow".into() },
                            when: calendar::when(e, hub.get_clock_24h()).into(),
                            title: e.title.as_str().into(),
                            location: e.location.as_str().into(),
                            now: !e.all_day && e.start <= now && e.end > now,
                        })
                        .collect();
                    let status = match (errors.first(), rows.is_empty()) {
                        (Some(err), _) => format!("Couldn't read a calendar: {err}"),
                        (None, true) => "Nothing else today or tomorrow".into(),
                        (None, false) => String::new(),
                    };
                    (rows, status)
                }
            };
            hub.set_calendar_status(status.into());
            set_rows(hub.get_calendar_events(), rows, |m| hub.set_calendar_events(m));
        }
    }

    // Notifications
    if seconds.is_multiple_of(2) || seconds == 1 {
        let (rows, status) = match app.notifications.1.lock().ok().and_then(|n| n.clone()) {
            None => (Vec::new(), "Reading notifications…".to_string()),
            Some(Ok(list)) => {
                let empty = list.is_empty();
                let rows = list
                    .into_iter()
                    .map(|n| NoticeRow {
                        id: n.id as i32,
                        app: n.app.into(),
                        title: n.title.into(),
                        body: n.body.into(),
                        time: n.time.into(),
                    })
                    .collect();
                (rows, if empty { "No notifications".into() } else { String::new() })
            }
            Some(Err(err)) => (Vec::new(), err),
        };
        hub.set_notices_status(status.into());
        set_rows(hub.get_notices(), rows, |m| hub.set_notices(m));
    }

    // Clipboard history, only while it's on.
    if hub.get_clipboard_history() {
        if app.clipboard.borrow_mut().poll() {
            show_clips(ui, &app.clipboard.borrow());
            hub.set_clips_status("".into());
        }
    } else if !app.clipboard.borrow().items.is_empty() {
        app.clipboard.borrow_mut().clear();
        show_clips(ui, &app.clipboard.borrow());
    }

    // Discord
    if let Some(state) = app.discord.1.lock().ok().map(|s| s.clone()) {
        hub.set_discord(DiscordInfo {
            status: state.status.as_str().into(),
            connected: state.connected,
            channel: state.channel.as_str().into(),
            mute: state.mute,
            deaf: state.deaf,
        });
        let members: Vec<VoiceMember> = state
            .members
            .iter()
            .map(|m| VoiceMember {
                name: m.name.as_str().into(),
                speaking: m.speaking,
                muted: m.muted,
            })
            .collect();
        set_rows(hub.get_discord_members(), members, |m| hub.set_discord_members(m));
    }

    // OBS
    if let Some(state) = app.obs.1.lock().ok().map(|s| s.clone()) {
        hub.set_obs(ObsInfo {
            status: state.status.as_str().into(),
            connected: state.connected,
            current: state.current.as_str().into(),
            recording: state.recording,
            record_paused: state.record_paused,
            record_time: state.record_time.as_str().into(),
            streaming: state.streaming,
            stream_time: state.stream_time.as_str().into(),
        });
        let scenes: Vec<slint::SharedString> = state.scenes.iter().map(|s| s.as_str().into()).collect();
        set_rows(hub.get_obs_scenes(), scenes, |m| hub.set_obs_scenes(m));
    }
}

/// One key on the on-screen keyboard. `mode` is how it types:
/// - "place": capitalizes the start of each word, as place names are
/// - "text": as typed
/// - "url": as typed, with no spaces, up to 300 characters
/// - "key": the key replaces the text, to pick one key for a combination
/// - "lower": returns the key in lower case (for showing lowercase key labels)
///
/// "space", "back" and "clear" do what they say.
fn edit_text(text: &str, key: &str, mode: &str) -> String {
    let max_chars = if mode == "url" { 300 } else { 40 };
    // A physical keyboard's special keys (arrows, F-keys) arrive as control or private-use
    // characters; they aren't text.
    let single = {
        let mut chars = key.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => Some(c),
            _ => None,
        }
    };
    if single.is_some_and(|c| c.is_control() || ('\u{E000}'..='\u{F8FF}').contains(&c)) {
        return text.to_string();
    }
    match (mode, key) {
        ("lower", key) => return key.to_lowercase(),
        ("key", "back" | "clear") => return String::new(),
        // Letters typed on a physical keyboard come in lower case; key names are capitals.
        ("key", key) if single.is_some() => return key.to_uppercase(),
        ("key", key) => return key.to_string(),
        _ => {}
    }
    let mut text = text.to_string();
    let full = text.chars().count() >= max_chars;
    match key {
        "back" => {
            text.pop();
        }
        "clear" => text.clear(),
        "space" if mode != "url" && !text.is_empty() && !text.ends_with(' ') && !full => text.push(' '),
        key if key.chars().count() == 1 && !full => {
            let starts_word = text.is_empty() || text.ends_with([' ', '-']);
            if mode == "place" {
                let key = if starts_word { key.to_uppercase() } else { key.to_lowercase() };
                text.push_str(&key);
            } else {
                text.push_str(key);
            }
        }
        _ => {}
    }
    text
}

/// Measures how long the configured server takes to answer, every 5 seconds in the
/// background, and shows the latest on the network card.
fn update_latency(app: &App, ui: &AppWindow, seconds: u64) {
    let hub = ui.global::<Hub>();
    let host = app.config.borrow().network.latency_host.trim().to_string();
    if !hub.get_show_latency() || host.is_empty() {
        hub.set_latency_text("".into());
        return;
    }
    if seconds.is_multiple_of(5) || seconds == 1 {
        let latest = app.latency.clone();
        std::thread::spawn(move || {
            let result = network::latency(&host);
            if let Ok(mut latest) = latest.lock() {
                *latest = Some(result);
            }
        });
    }
    let text = match app.latency.lock().ok().and_then(|l| l.clone()) {
        Some(Ok(time)) => format!("{} ms", time.as_millis().max(1)),
        Some(Err(_)) => "no answer".into(),
        None => String::new(),
    };
    hub.set_latency_text(text.into());
}

/// How often to look for a newer release, in seconds.
const UPDATE_CHECK_EVERY_SECS: u64 = 24 * 60 * 60;

/// Looks for a newer release on GitHub soon after starting and then once a day, and says
/// how that went in Customize hub.
fn update_check(app: &App, ui: &AppWindow, seconds: u64) {
    let hub = ui.global::<Hub>();
    let current = env!("CARGO_PKG_VERSION");
    if !hub.get_check_updates() {
        hub.set_update_status(format!("Version {current}; not checking for updates").into());
        hub.set_update_available(false);
        return;
    }
    // A little after starting, so it doesn't compete with everything else starting up.
    if seconds == 15 || seconds.is_multiple_of(UPDATE_CHECK_EVERY_SECS) {
        let latest = app.update.clone();
        std::thread::spawn(move || {
            let result = web::latest_release();
            if let Err(err) = &result {
                eprintln!("updates: {err}");
            }
            if let Ok(mut latest) = latest.lock() {
                *latest = Some(result);
            }
        });
    }
    let (status, available) = match app.update.lock().ok().and_then(|u| u.clone()) {
        None => (format!("Version {current}; checking soon"), false),
        Some(Ok(Some((version, _)))) if web::is_newer(&version, current) => {
            (format!("Version {version} is out (you have {current})"), true)
        }
        Some(Ok(_)) => (format!("Version {current}, the newest"), false),
        Some(Err(err)) => (format!("Version {current}; couldn't check: {err}"), false),
    };
    hub.set_update_status(status.into());
    hub.set_update_available(available);
}

/// The battery in the top bar, on PCs that have one.
fn show_battery(ui: &AppWindow) {
    let hub = ui.global::<Hub>();
    match stats::battery() {
        Some((percent, charging)) => {
            hub.set_battery_text(format!("{percent}%").into());
            hub.set_battery_charging(charging);
            hub.set_battery_low(percent <= 20);
        }
        None => hub.set_battery_text("".into()),
    }
}

/// How often the weather is looked up again.
const WEATHER_EVERY_SECS: u64 = 15 * 60;

/// Looks the weather up in the background when it's due, and shows the latest in the
/// top bar in the chosen temperature unit, and how the lookup went in Customize hub.
fn update_weather(app: &App, ui: &AppWindow, seconds: u64) {
    let place = app.weather_config();
    let hub = ui.global::<Hub>();
    if !place.is_set() {
        hub.set_weather_text("".into());
        hub.set_weather_status("Not set: tap Set place and type a town or city".into());
        return;
    }
    // Looked up even while hidden, so turning it on shows it at once.
    if app.weather_due.replace(false) || seconds.is_multiple_of(WEATHER_EVERY_SECS) {
        let latest = app.weather.clone();
        std::thread::spawn(move || {
            let result = web::weather(&place);
            if let Err(err) = &result {
                eprintln!("weather: {err}");
            }
            if let Ok(mut latest) = latest.lock() {
                *latest = Some(result);
            }
        });
    }
    let latest = app.weather.lock().ok().and_then(|w| w.clone());
    let conditions = |w: &web::Weather| {
        let degrees = if hub.get_fahrenheit() { w.celsius * 9.0 / 5.0 + 32.0 } else { w.celsius };
        format!("{degrees:.0}° {}", w.summary).trim().to_string()
    };
    let (text, status) = match &latest {
        None => (String::new(), "Looking it up…".to_string()),
        Some(Ok(weather)) => {
            let place = if weather.place.is_empty() { "Your coordinates" } else { weather.place.as_str() };
            (conditions(weather), format!("{place} · {}", conditions(weather)))
        }
        Some(Err(err)) => (String::new(), format!("Couldn't get the weather: {err}")),
    };
    hub.set_weather_text(if hub.get_show_weather() { text } else { String::new() }.into());
    hub.set_weather_status(status.into());
    if hub.get_weather_open() {
        if let Some(Ok(weather)) = &latest {
            show_weather_details(ui, weather);
        }
    }
}

/// Fills the weather dialog, in the temperature unit picked on the hub (with mph for °F).
fn show_weather_details(ui: &AppWindow, weather: &web::Weather) {
    let hub = ui.global::<Hub>();
    let fahrenheit = hub.get_fahrenheit();
    let degrees = |celsius: f32| {
        let value = if fahrenheit { celsius * 9.0 / 5.0 + 32.0 } else { celsius };
        format!("{value:.0}°")
    };
    let wind = if fahrenheit {
        format!("{:.0} mph", weather.wind_kmh / 1.609)
    } else {
        format!("{:.0} km/h", weather.wind_kmh)
    };
    let today = weather.days.first();
    hub.set_weather_details(WeatherDetails {
        place: weather.place.as_str().into(),
        temperature: degrees(weather.celsius).into(),
        summary: weather.summary.into(),
        feels_like: degrees(weather.feels_like).into(),
        humidity: format!("{}%", weather.humidity).into(),
        wind: wind.into(),
        high_low: today.map(|d| format!("{} / {}", degrees(d.high), degrees(d.low))).unwrap_or_default().into(),
        rain_chance: today.map(|d| format!("{}%", d.rain_chance)).unwrap_or_default().into(),
        sunrise: weather.sunrise.as_str().into(),
        sunset: weather.sunset.as_str().into(),
    });
    let days: Vec<ForecastDay> = weather
        .days
        .iter()
        .enumerate()
        .map(|(i, day)| ForecastDay {
            day: match i {
                0 => "Today".to_string(),
                1 => "Tomorrow".to_string(),
                _ => chrono::NaiveDate::parse_from_str(&day.date, "%Y-%m-%d")
                    .map(|date| date.format("%A").to_string())
                    .unwrap_or_else(|_| day.date.clone()),
            }
            .into(),
            summary: day.summary.into(),
            high: degrees(day.high).into(),
            low: degrees(day.low).into(),
            rain_chance: format!("{}% rain", day.rain_chance).into(),
        })
        .collect();
    // Only replace the rows when they change, since this runs every second while open.
    let model = hub.get_forecast();
    let same = model.row_count() == days.len() && days.iter().enumerate().all(|(i, d)| model.row_data(i).as_ref() == Some(d));
    if !same {
        hub.set_forecast(ModelRc::from(Rc::new(VecModel::from(days))));
    }
}

/// Light or dark, from the Theme setting (and Windows, when following it).
fn apply_theme(ui: &AppWindow) {
    let light = match ui.global::<Hub>().get_theme() {
        1 => true,
        2 => actions::windows_light_theme(),
        _ => false,
    };
    let palette = ui.global::<Palette>();
    if palette.get_light() != light {
        palette.set_light(light);
    }
}

fn apply_settings(ui: &AppWindow, s: &settings::Settings, defaults: Defaults) {
    let accent = s
        .accent
        .as_deref()
        .and_then(config::parse_color)
        .unwrap_or(defaults.accent);
    ui.global::<Palette>().set_chosen(accent);

    let hub = ui.global::<Hub>();
    let custom = s.custom_accent.as_deref().filter(|c| config::parse_color(c).is_some());
    hub.set_custom_accent(custom.map(|c| c.trim().trim_start_matches('#').to_uppercase()).unwrap_or_default().into());
    hub.set_clock_24h(s.clock_24h);
    hub.set_show_date(s.show_date);
    hub.set_show_uptime(s.show_uptime);
    hub.set_theme(settings::index_of(&settings::THEMES, &s.theme, 0) as i32);
    hub.set_show_weather(s.show_weather);
    hub.set_weather_place(s.weather_place.clone().unwrap_or_default().into());
    hub.set_show_music(s.show_music);
    hub.set_show_monitor(s.show_monitor);
    hub.set_show_shortcuts(s.show_shortcuts);
    hub.set_show_widgets(s.show_widgets);
    hub.set_language(settings::index_of(&settings::LANGUAGES, &s.language, 0) as i32);
    apply_language(&s.language);
    hub.set_art_accent(s.art_accent);
    hub.set_show_sensor_cards(s.show_sensor_cards);
    hub.set_show_device_batteries(s.show_device_batteries);
    hub.set_show_mic_meter(s.show_mic_meter);
    hub.set_show_calendar(s.show_calendar);
    hub.set_show_notifications(s.show_notifications);
    hub.set_clipboard_history(s.clipboard_history);
    hub.set_show_clipboard_card(s.show_clipboard);
    hub.set_show_discord(s.show_discord);
    hub.set_show_obs(s.show_obs);
    hub.set_hardware_brightness(s.hardware_brightness);
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
    hub.set_show_history(s.show_history);
    hub.set_show_latency(s.show_latency);
    hub.set_show_album_art(s.show_album_art);
    hub.set_show_outputs(s.show_outputs);
    hub.set_show_inputs(s.show_inputs);
    hub.set_show_key_hints(s.show_key_hints);
    hub.set_brightness(s.brightness as i32);
    hub.set_idle_dim(s.dim_when_idle);
    hub.set_idle_minutes(s.idle_minutes as i32);
    hub.set_dark_when_locked(s.dark_when_locked);
    hub.set_idle_clock(s.idle_clock);
    hub.set_check_updates(s.check_updates);
    hub.set_fullscreen(s.fullscreen.unwrap_or(defaults.fullscreen));
    update_clock(ui);
    apply_theme(ui);
}

fn read_settings(
    ui: &AppWindow,
    defaults: Defaults,
    window: Option<[i32; 4]>,
    screen: Option<[i32; 2]>,
) -> settings::Settings {
    let hub = ui.global::<Hub>();
    let accent = ui.global::<Palette>().get_chosen();
    let page = |i: i32| settings::PAGES[i.clamp(0, 3) as usize].to_string();
    settings::Settings {
        accent: (accent != defaults.accent).then(|| {
            format!("#{:02X}{:02X}{:02X}", accent.red(), accent.green(), accent.blue())
        }),
        custom_accent: Some(hub.get_custom_accent().to_string())
            .filter(|c| !c.is_empty())
            .map(|c| format!("#{c}")),
        clock_24h: hub.get_clock_24h(),
        show_date: hub.get_show_date(),
        show_uptime: hub.get_show_uptime(),
        theme: settings::THEMES[hub.get_theme().clamp(0, 2) as usize].into(),
        show_weather: hub.get_show_weather(),
        // Filled in by App::save_settings, which knows the place.
        weather_place: None,
        show_music: hub.get_show_music(),
        show_monitor: hub.get_show_monitor(),
        show_shortcuts: hub.get_show_shortcuts(),
        show_widgets: hub.get_show_widgets(),
        language: settings::LANGUAGES[hub.get_language().clamp(0, settings::LANGUAGES.len() as i32 - 1) as usize].into(),
        art_accent: hub.get_art_accent(),
        show_sensor_cards: hub.get_show_sensor_cards(),
        show_device_batteries: hub.get_show_device_batteries(),
        show_mic_meter: hub.get_show_mic_meter(),
        show_calendar: hub.get_show_calendar(),
        show_notifications: hub.get_show_notifications(),
        clipboard_history: hub.get_clipboard_history(),
        show_clipboard: hub.get_show_clipboard_card(),
        show_discord: hub.get_show_discord(),
        show_obs: hub.get_show_obs(),
        hardware_brightness: hub.get_hardware_brightness(),
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
        show_history: hub.get_show_history(),
        show_latency: hub.get_show_latency(),
        show_album_art: hub.get_show_album_art(),
        show_outputs: hub.get_show_outputs(),
        show_inputs: hub.get_show_inputs(),
        show_key_hints: hub.get_show_key_hints(),
        // Filled in by App::save_settings, which knows the page.
        shortcut_page: None,
        brightness: hub.get_brightness().max(0) as u32,
        dim_when_idle: hub.get_idle_dim(),
        idle_minutes: hub.get_idle_minutes().max(1) as u32,
        dark_when_locked: hub.get_dark_when_locked(),
        idle_clock: hub.get_idle_clock(),
        check_updates: hub.get_check_updates(),
        fullscreen: (hub.get_fullscreen() != defaults.fullscreen).then_some(hub.get_fullscreen()),
        window,
        screen,
    }
    .sanitized()
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Picks up edits to hub.toml without a restart.
fn watch_config(app: &Rc<App>) -> Timer {
    let timer = Timer::default();
    let Some(path) = app.config_path.clone() else { return timer };
    let app = app.clone();
    timer.start(TimerMode::Repeated, Duration::from_secs(2), move || {
        let now = modified(&path);
        if now == app.config_stamp.get() {
            return;
        }
        app.config_stamp.set(now);
        let Some(ui) = app.ui.upgrade() else { return };
        let message = match config::read(&path) {
            Ok(config) => {
                app.apply_config(&ui, config);
                "Reloaded hub.toml"
            }
            Err(err) => {
                eprintln!("config: {} has an error:\n{err}", path.display());
                "hub.toml has an error; see pc-hub.log or the console"
            }
        };
        ui.global::<Hub>().set_last_action(message.into());
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
    cpu_history: History,
    gpu_history: History,
    /// MB/s
    down_history: History,
    up_history: History,
    /// Since when each temperature has been above the heat warning.
    cpu_hot_since: Option<Instant>,
    gpu_hot_since: Option<Instant>,
    /// Today's downloads and uploads, saved to `usage_path` now and then.
    usage: network::DailyUsage,
    usage_path: PathBuf,
    usage_saved: Instant,
    /// Everything HWiNFO reported last, for picking readings for cards.
    snapshot: Option<hwinfo::Snapshot>,
    /// The graphics card's name in the summary, once known (HWiNFO may start later).
    gpu_name: Option<String>,
}

/// How long a temperature stays above the heat warning before its card pulses.
const HEAT_ALARM_AFTER: Duration = Duration::from_secs(30);

impl Monitor {
    fn new(usage_path: PathBuf) -> Self {
        Self {
            usage: network::DailyUsage::load(&usage_path),
            usage_path,
            usage_saved: Instant::now(),
            stats: stats::Stats::new(),
            gpu: gpu::Gpu::new(),
            cpu_history: History::default(),
            gpu_history: History::default(),
            down_history: History::default(),
            up_history: History::default(),
            cpu_hot_since: None,
            gpu_hot_since: None,
            snapshot: None,
            gpu_name: None,
        }
    }

    /// Fills the processes sheet with the busiest apps.
    fn refresh_top_apps(&mut self, ui: &AppWindow) {
        let hub = ui.global::<Hub>();
        let by_memory = hub.get_processes_by_memory();
        let total_memory = self.stats.ram_total_gb() * GIB;
        let rows: Vec<AppUsageRow> = self
            .stats
            .top_apps(12, by_memory)
            .into_iter()
            .map(|app| {
                let memory_gb = app.memory as f64 / GIB;
                AppUsageRow {
                    name: app.name.into(),
                    cpu: format!("{:.0}%", app.cpu * 100.0).into(),
                    memory: if memory_gb >= 1.0 {
                        format!("{memory_gb:.1} GB")
                    } else {
                        format!("{:.0} MB", app.memory as f64 / (1024.0 * 1024.0))
                    }
                    .into(),
                    share: if by_memory {
                        (app.memory as f64 / total_memory.max(1.0)) as f32
                    } else {
                        app.cpu
                    },
                }
            })
            .collect();
        hub.set_top_apps(ModelRc::from(Rc::new(VecModel::from(rows))));
    }

    /// e.g. "Ryzen 7 7800X3D · RTX 5070 · 32 GB RAM"
    fn summary(&self) -> String {
        let mut parts = vec![self.stats.cpu_name()];
        parts.extend(self.gpu.name().or(self.gpu_name.clone()));
        parts.push(format!("{} GB RAM", self.stats.ram_total_gb()));
        parts.retain(|p| !p.is_empty());
        parts.join(" · ")
    }

    fn refresh(&mut self, ui: &AppWindow, cpu_fan: Option<&str>, drives: &[String], cards: &[config::SensorCard]) {
        let hub = ui.global::<Hub>();
        let fahrenheit = hub.get_fahrenheit();
        let warning = hub.get_heat_warning() as f32;
        let temp = |celsius: Option<f32>| match celsius {
            Some(c) if fahrenheit => format!("{:.0}°F", c * 9.0 / 5.0 + 32.0),
            Some(c) => format!("{c:.0}°C"),
            None => "—".into(),
        };
        let or_dash = |value: Option<String>| value.unwrap_or_else(|| "—".into());

        let sample = self.stats.sample(drives);
        hub.set_uptime_text(format!("Up {}", stats::format_uptime(sample.uptime_secs)).into());

        // Processor: load from Windows, the rest from HWiNFO when it's running.
        self.snapshot = hwinfo::read();
        hub.set_cpu_sensors_missing(self.snapshot.is_none());
        let sensors = self.snapshot.as_ref().map(|s| s.cpu(cpu_fan)).unwrap_or_default();
        hub.set_cpu_load(sample.cpu);
        hub.set_cpu_temp(temp(sensors.temp_c).into());
        let cpu_hot = sensors.temp_c.is_some_and(|t| t > warning);
        hub.set_cpu_temp_hot(cpu_hot);
        hub.set_cpu_alarm(heat_alarm(&mut self.cpu_hot_since, cpu_hot));
        let mhz = sensors.clock_mhz.or((sample.cpu_mhz > 0).then_some(sample.cpu_mhz as f32));
        hub.set_cpu_clock(or_dash(mhz.map(|m| format!("{:.2}", m / 1000.0))).into());
        hub.set_cpu_power(or_dash(sensors.power_w.map(|w| format!("{w:.0}"))).into());
        hub.set_cpu_fan(or_dash(sensors.fan_rpm.map(|r| thousands(r as u64))).into());

        // Graphics card: NVIDIA's own library when it's an NVIDIA card, HWiNFO for the rest
        // (and for the hot spot and memory temperatures NVIDIA's library doesn't give).
        let nvidia = self.gpu.sample();
        let n = nvidia.as_ref();
        let h = self.snapshot.as_ref().map(|s| s.gpu()).unwrap_or_default();
        if self.gpu.name().is_none() && h.name.is_some() && h.name != self.gpu_name {
            self.gpu_name = h.name.clone();
            hub.set_system_summary(self.summary().into());
        }
        let gpu_load = n.map(|g| g.load).or(h.load).unwrap_or(0.0);
        let gpu_temp = n.and_then(|g| g.temp_c).or(h.temp_c);
        hub.set_gpu_load(gpu_load);
        hub.set_gpu_temp(temp(gpu_temp).into());
        let gpu_hot = gpu_temp.is_some_and(|t| t > warning);
        hub.set_gpu_temp_hot(gpu_hot);
        hub.set_gpu_alarm(heat_alarm(&mut self.gpu_hot_since, gpu_hot));
        let clock = n.and_then(|g| g.clock_mhz.map(|c| c as f32)).or(h.clock_mhz);
        hub.set_gpu_clock(or_dash(clock.map(|c| thousands(c as u64))).into());
        let power = n.and_then(|g| g.power_w).or(h.power_w);
        hub.set_gpu_power(or_dash(power.map(|w| format!("{w:.0}"))).into());
        let fan = n.and_then(|g| g.fan.map(|(v, rpm)| (v as f32, rpm))).or(h.fan);
        hub.set_gpu_fan(or_dash(fan.map(|(v, _)| thousands(v as u64))).into());
        hub.set_gpu_fan_unit(if fan.is_some_and(|(_, rpm)| !rpm) { "%" } else { "RPM" }.into());
        // Extra temperatures and the game's frame rate, under the readings.
        let mut note = Vec::new();
        if let Some(c) = h.hotspot_c {
            note.push(format!("Hot spot {}", temp(Some(c))));
        }
        if let Some(c) = h.memory_c {
            note.push(format!("Memory {}", temp(Some(c))));
        }
        if let Some(fps) = rtss::read() {
            note.push(format!("{:.0} FPS · {}", fps.fps, fps.program));
        }
        hub.set_gpu_note(note.join(" · ").into());

        // Cards for readings chosen in hub.toml.
        let rows: Vec<SensorCardRow> = cards
            .iter()
            .map(|card| {
                let reading = self.snapshot.as_ref().and_then(|s| s.find(card.sensor.as_deref(), &card.reading));
                let (value, unit) = reading.map(|r| hwinfo::display(r, fahrenheit)).unwrap_or(("—".into(), String::new()));
                SensorCardRow {
                    label: if card.label.trim().is_empty() { card.reading.as_str() } else { card.label.as_str() }.into(),
                    value: value.into(),
                    unit: unit.into(),
                    fraction: match (reading, card.max) {
                        (Some(r), Some(max)) if max > 0.0 => (r.value / max).clamp(0.0, 1.0) as f32,
                        _ => 0.0,
                    },
                    has_meter: card.max.is_some_and(|m| m > 0.0),
                }
            })
            .collect();
        let model = hub.get_sensor_cards();
        if model.row_count() == rows.len() {
            for (i, row) in rows.into_iter().enumerate() {
                model.set_row_data(i, row);
            }
        } else {
            hub.set_sensor_cards(ModelRc::from(Rc::new(VecModel::from(rows))));
        }
        let g = n;

        // Memory, video memory, network, disk
        hub.set_ram_value(format!("{:.1}", sample.ram_used as f64 / GIB).into());
        hub.set_ram_unit(format!(" / {} GB", self.stats.ram_total_gb()).into());
        hub.set_ram_usage(fraction(sample.ram_used, sample.ram_total));

        match g.and_then(|g| g.vram).or(h.vram) {
            Some((used, total)) => {
                hub.set_vram_value(format!("{:.1}", used as f64 / GIB).into());
                hub.set_vram_unit(format!(" / {:.0} GB", total as f64 / GIB).into());
                hub.set_vram_usage(fraction(used, total));
            }
            None => hub.set_vram_value("—".into()),
        }

        hub.set_net_down(format!("{:.1}", sample.net_down / 1e6).into());
        hub.set_net_up(format!("{:.1}", sample.net_up / 1e6).into());
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        self.usage.add(&today, sample.received, sample.sent);
        hub.set_net_today(
            format!("Today ↓ {} ↑ {}", network::bytes(self.usage.received), network::bytes(self.usage.sent)).into(),
        );
        hub.set_net_month(
            format!("Month ↓ {} ↑ {}", network::bytes(self.usage.month_received), network::bytes(self.usage.month_sent)).into(),
        );
        if self.usage_saved.elapsed() >= Duration::from_secs(60) {
            self.usage.save(&self.usage_path);
            self.usage_saved = Instant::now();
        }

        let drives: Vec<DriveUsage> = sample
            .drives
            .iter()
            .map(|d| DriveUsage {
                label: d.name.as_str().into(),
                value: format!("{:.0}", d.used as f64 / GIB).into(),
                unit: format!(" / {:.0} GB", d.total as f64 / GIB).into(),
                usage: fraction(d.used, d.total),
            })
            .collect();
        // Update the rows in place when the same drives are shown, so their meters animate.
        let model = hub.get_drives();
        if model.row_count() == drives.len() {
            for (i, drive) in drives.into_iter().enumerate() {
                model.set_row_data(i, drive);
            }
        } else {
            hub.set_drives(ModelRc::from(Rc::new(VecModel::from(drives))));
        }

        // History graphs keep recording while hidden, so they're full when turned on.
        self.cpu_history.push(sample.cpu);
        self.gpu_history.push(gpu_load);
        self.down_history.push((sample.net_down / 1e6) as f32);
        self.up_history.push((sample.net_up / 1e6) as f32);
        if hub.get_show_history() {
            let graph = |history: &History, top: f32| {
                let (line, area) = history.graph(top);
                Graph {
                    line: line.into(),
                    area: area.into(),
                }
            };
            hub.set_cpu_graph(graph(&self.cpu_history, 1.0));
            hub.set_gpu_graph(graph(&self.gpu_history, 1.0));
            // Network graphs share a scale, at least 0.5 MB/s so an idle line stays low.
            let top = self.down_history.max().max(self.up_history.max()).max(0.5);
            hub.set_net_down_graph(graph(&self.down_history, top));
            hub.set_net_up_graph(graph(&self.up_history, top));
        }
    }
}

/// Starts or clears a heat timer. True once it has run longer than HEAT_ALARM_AFTER.
fn heat_alarm(since: &mut Option<Instant>, hot: bool) -> bool {
    if !hot {
        *since = None;
        return false;
    }
    since.get_or_insert_with(Instant::now).elapsed() >= HEAT_ALARM_AFTER
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
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::edit_text;

    #[test]
    fn typing_a_place() {
        let mut text = String::new();
        for key in ["N", "E", "W", "space", "Y", "O", "R", "K"] {
            text = edit_text(&text, key, "place");
        }
        assert_eq!(text, "New York");
        assert_eq!(edit_text(&text, "back", "place"), "New Yor");
        assert_eq!(edit_text(&text, "clear", "place"), "");
        // No leading or doubled spaces.
        assert_eq!(edit_text("", "space", "place"), "");
        assert_eq!(edit_text("New ", "space", "place"), "New ");
        assert_eq!(edit_text(&"A".repeat(40), "B", "place").len(), 40);
    }

    #[test]
    fn typing_other_things() {
        assert_eq!(edit_text("OB", "S", "text"), "OBS");
        assert_eq!(edit_text("https://", "e", "url"), "https://e");
        assert_eq!(edit_text("https://e", "space", "url"), "https://e");
        assert_eq!(edit_text("F4", "F10", "key"), "F10");
        assert_eq!(edit_text("F10", "back", "key"), "");
        assert_eq!(edit_text("", "Q", "lower"), "q");
        // From a physical keyboard: lower-case letters for keys, and special keys ignored.
        assert_eq!(edit_text("", "m", "key"), "M");
        assert_eq!(edit_text("New", "\u{F704}", "place"), "New");
        assert_eq!(edit_text("New", "\u{8}", "text"), "New");
    }
}
