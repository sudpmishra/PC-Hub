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

    // Windows
    pub show_music: bool,
    pub show_monitor: bool,
    pub show_shortcuts: bool,
    /// "music", "monitor" or "shortcuts"
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

    // Music
    pub show_album_art: bool,
    pub show_outputs: bool,
    pub show_inputs: bool,

    // Shortcuts
    pub show_key_hints: bool,

    // Display & power
    /// 30 to 100 percent.
    pub brightness: u32,
    pub dim_when_idle: bool,
    pub idle_minutes: u32,
    pub dark_when_locked: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            accent: None,
            custom_accent: None,
            clock_24h: true,
            show_date: true,
            show_uptime: true,
            show_music: true,
            show_monitor: true,
            show_shortcuts: true,
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
            show_album_art: true,
            show_outputs: true,
            show_inputs: true,
            show_key_hints: true,
            brightness: 100,
            dim_when_idle: true,
            idle_minutes: 5,
            dark_when_locked: true,
        }
    }
}

/// Pages in the order they appear; the UI refers to them by index.
pub const PAGES: [&str; 3] = ["music", "monitor", "shortcuts"];
pub const ANIMATIONS: [&str; 3] = ["off", "fast", "smooth"];

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
        if !(self.show_music || self.show_monitor || self.show_shortcuts) {
            self.show_monitor = true;
        }
        self
    }
}

pub fn save(path: &Path, settings: &Settings) {
    let result = toml::to_string(settings)
        .map_err(|e| e.to_string())
        .and_then(|text| std::fs::write(path, text).map_err(|e| e.to_string()));
    if let Err(err) = result {
        eprintln!("settings: could not save {}: {err}", path.display());
    }
}
