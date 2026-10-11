//! Choices made on the hub's Settings sheet and Customize hub screen, saved to
//! hub-settings.toml next to hub.toml so hand-written comments in hub.toml are never rewritten.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "hub-settings.toml";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    // Appearance
    /// "#RRGGBB" picked on the hub; none means the accent from hub.toml.
    pub accent: Option<String>,
    /// "#RRGGBB" typed on the hub's keypad, kept even while a preset accent is picked.
    pub custom_accent: Option<String>,
    pub clock_24h: bool,
    pub show_date: bool,
    pub show_uptime: bool,
    /// "dark", "light" or "system" (follow the Windows app theme).
    pub theme: String,
    /// The weather in the top bar, for weather_place or else [weather] in hub.toml.
    pub show_weather: bool,
    /// A place typed on the hub, such as "Amsterdam".
    pub weather_place: Option<String>,
    /// "system" follows Windows; otherwise a language code such as "de".
    pub language: String,
    /// While music plays, take the accent from the album art.
    pub art_accent: bool,

    // Windows
    pub show_music: bool,
    pub show_monitor: bool,
    pub show_shortcuts: bool,
    pub show_widgets: bool,
    /// "music", "monitor", "shortcuts" or "widgets"
    pub start_page: String,
    pub swipe: bool,
    /// "off", "fast" or "smooth"
    pub slide_animation: String,

    // Hardware monitor
    pub refresh_seconds: u32,
    pub fahrenheit: bool,
    /// Temperatures above this (in °C) turn amber.
    pub heat_warning: u32,
    pub show_memory: bool,
    pub show_video_memory: bool,
    pub show_network: bool,
    pub show_storage: bool,
    /// Graphs of the last minute or so behind the processor, graphics card and network readings.
    pub show_history: bool,
    /// How long a server takes to answer, on the network card.
    pub show_latency: bool,
    /// Cards for HWiNFO readings chosen in hub.toml ([[card]]).
    pub show_sensor_cards: bool,
    /// Battery levels of wireless headsets, controllers and the like.
    pub show_device_batteries: bool,

    // Music
    pub show_album_art: bool,
    pub show_outputs: bool,
    pub show_inputs: bool,
    /// A live level meter for the microphone under Input.
    pub show_mic_meter: bool,

    // Widgets page
    pub show_calendar: bool,
    pub show_notifications: bool,
    /// Remember copied text (in memory only) to paste it again from the hub.
    pub clipboard_history: bool,
    pub show_clipboard: bool,
    pub show_discord: bool,
    pub show_obs: bool,

    // Shortcuts
    pub show_key_hints: bool,
    /// The shortcut page last shown, when hub.toml has pages.
    pub shortcut_page: Option<String>,

    // Display & power
    /// 30 to 100 percent.
    pub brightness: u32,
    /// Change the monitor's own backlight (DDC/CI) instead of darkening the picture.
    pub hardware_brightness: bool,
    pub dim_when_idle: bool,
    pub idle_minutes: u32,
    pub dark_when_locked: bool,
    /// Show a dim clock while the screen is dimmed for being idle.
    pub idle_clock: bool,
    /// Look for a newer release on GitHub once a day.
    pub check_updates: bool,
    /// None follows `fullscreen` in hub.toml.
    pub fullscreen: Option<bool>,
    /// Where the window was last left when not full screen: x, y, width, height in
    /// physical pixels. None opens it where hub.toml says.
    pub window: Option<[i32; 4]>,
    /// Top-left corner of the screen picked on the hub for full screen, in physical pixels.
    /// None uses x and y from hub.toml.
    pub screen: Option<[i32; 2]>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            accent: None,
            custom_accent: None,
            clock_24h: true,
            show_date: true,
            show_uptime: true,
            theme: "dark".into(),
            show_weather: true,
            weather_place: None,
            language: "system".into(),
            art_accent: false,
            show_music: true,
            show_monitor: true,
            show_shortcuts: true,
            show_widgets: true,
            start_page: "monitor".into(),
            swipe: true,
            slide_animation: "smooth".into(),
            refresh_seconds: 1,
            fahrenheit: false,
            heat_warning: 80,
            show_memory: true,
            show_video_memory: true,
            show_network: true,
            show_storage: true,
            show_history: true,
            show_latency: true,
            show_sensor_cards: true,
            show_device_batteries: true,
            show_album_art: true,
            show_outputs: true,
            show_inputs: true,
            show_mic_meter: true,
            show_calendar: true,
            show_notifications: true,
            clipboard_history: false,
            show_clipboard: true,
            show_discord: true,
            show_obs: true,
            show_key_hints: true,
            shortcut_page: None,
            brightness: 100,
            hardware_brightness: false,
            dim_when_idle: true,
            idle_minutes: 5,
            dark_when_locked: true,
            idle_clock: true,
            check_updates: true,
            fullscreen: None,
            window: None,
            screen: None,
        }
    }
}

/// Pages in the order they appear; the UI refers to them by index.
pub const PAGES: [&str; 4] = ["music", "monitor", "shortcuts", "widgets"];
/// Languages the interface is translated into, after "system" (follow Windows) and English.
pub const LANGUAGES: [&str; 5] = ["system", "en", "de", "es", "fr"];
pub const ANIMATIONS: [&str; 3] = ["off", "fast", "smooth"];
pub const THEMES: [&str; 3] = ["dark", "light", "system"];

pub fn index_of(names: &[&str], name: &str, fallback: usize) -> usize {
    names.iter().position(|n| *n == name).unwrap_or(fallback)
}

pub fn path_beside(config_path: Option<&Path>) -> PathBuf {
    config_path
        .and_then(Path::parent)
        .map(|dir| dir.join(FILE_NAME))
        .unwrap_or_else(|| PathBuf::from(FILE_NAME))
}

pub fn load(path: &Path) -> Settings {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Settings::default();
    };
    let settings: Settings = toml::from_str(&text).unwrap_or_else(|err| {
        eprintln!("settings: {} has an error, using defaults:\n{err}", path.display());
        Settings::default()
    });
    settings.sanitized()
}

impl Settings {
    /// Keeps hand-edited values within what the hub can show.
    pub fn sanitized(mut self) -> Self {
        self.brightness = self.brightness.clamp(30, 100);
        self.refresh_seconds = self.refresh_seconds.clamp(1, 60);
        self.heat_warning = self.heat_warning.clamp(50, 105);
        self.idle_minutes = self.idle_minutes.clamp(1, 240);
        if !(self.show_music || self.show_monitor || self.show_shortcuts || self.show_widgets) {
            self.show_monitor = true;
        }
        // A window too small to grab would be hard to get back.
        self.window = self.window.filter(|[_, _, w, h]| *w >= MIN_WINDOW[0] && *h >= MIN_WINDOW[1]);
        self
    }
}

/// The smallest window the corner grip allows, in physical pixels.
pub const MIN_WINDOW: [i32; 2] = [480, 300];

pub fn save(path: &Path, settings: &Settings) {
    let result = toml::to_string(settings)
        .map_err(|e| e.to_string())
        .and_then(|text| std::fs::write(path, text).map_err(|e| e.to_string()));
    if let Err(err) = result {
        eprintln!("settings: could not save {}: {err}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitized_keeps_values_in_range() {
        let s = Settings {
            brightness: 5,
            refresh_seconds: 0,
            heat_warning: 200,
            idle_minutes: 0,
            show_music: false,
            show_monitor: false,
            show_shortcuts: false,
            show_widgets: false,
            window: Some([0, 0, 100, 100]),
            ..Settings::default()
        }
        .sanitized();
        assert_eq!(s.brightness, 30);
        assert_eq!(s.refresh_seconds, 1);
        assert_eq!(s.heat_warning, 105);
        assert_eq!(s.idle_minutes, 1);
        assert!(s.show_monitor);
        assert_eq!(s.window, None);
    }

    #[test]
    fn round_trips_through_toml() {
        let s = Settings {
            fullscreen: Some(false),
            window: Some([1920, 0, 1024, 600]),
            accent: Some("#FF0000".into()),
            ..Settings::default()
        };
        let back: Settings = toml::from_str(&toml::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.fullscreen, Some(false));
        assert_eq!(back.window, Some([1920, 0, 1024, 600]));
        assert_eq!(back.accent.as_deref(), Some("#FF0000"));
    }

    #[test]
    fn older_files_still_load() {
        let s: Settings = toml::from_str("clock_24h = false\n").unwrap();
        assert!(!s.clock_24h);
        assert_eq!(s.fullscreen, None);
        assert_eq!(s.start_page, "monitor");
    }
}
