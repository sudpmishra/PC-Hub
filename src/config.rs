//! Loads hub.toml. The file is looked up next to the executable first, then in the
//! current directory (handy for `cargo run`). If neither exists, a default copy is
//! written next to the executable so there is always something to edit.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::actions;

const FILE_NAME: &str = "hub.toml";
const DEFAULT_CONFIG: &str = include_str!("../hub.toml");

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub columns: u32,
    /// Accent color as "#RRGGBB".
    pub accent: Option<String>,
    pub window: WindowConfig,
    pub sensors: SensorsConfig,
    #[serde(rename = "shortcut")]
    pub shortcuts: Vec<ShortcutConfig>,
    #[serde(rename = "output")]
    pub outputs: Vec<DeviceConfig>,
    #[serde(rename = "input")]
    pub inputs: Vec<DeviceConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            columns: 5,
            accent: None,
            window: WindowConfig::default(),
            sensors: SensorsConfig::default(),
            shortcuts: Vec::new(),
            outputs: Vec::new(),
            inputs: Vec::new(),
        }
    }
}

impl Config {
    pub fn accent_color(&self) -> Option<slint::Color> {
        parse_color(self.accent.as_deref()?)
    }
}

/// "#RRGGBB" to a color.
pub fn parse_color(text: &str) -> Option<slint::Color> {
    let hex = text.trim().trim_start_matches('#');
    let value = u32::from_str_radix(hex, 16).ok().filter(|_| hex.len() == 6)?;
    Some(slint::Color::from_rgb_u8(
        (value >> 16) as u8,
        (value >> 8) as u8,
        value as u8,
    ))
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct WindowConfig {
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub fullscreen: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct SensorsConfig {
    /// HWiNFO's name for the processor fan, for boards that don't label it "CPU".
    pub cpu_fan: Option<String>,
}

/// One button on the Shortcuts page. It does one of three things, checked in this
/// order: presses `keys`, performs a built-in `action`, or opens `run`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ShortcutConfig {
    pub label: String,
    pub icon: Option<String>,
    /// Small text under the label; defaults to the key combination or "Launch app".
    pub hint: Option<String>,
    pub keys: Option<String>,
    pub action: Option<String>,
    pub run: String,
    pub args: Vec<String>,
}

impl ShortcutConfig {
    pub fn hint(&self) -> String {
        if let Some(hint) = &self.hint {
            return hint.clone();
        }
        if let Some(keys) = &self.keys {
            return keys.clone();
        }
        match self.action.as_deref() {
            Some("sleep") => "Power action".into(),
            Some(_) => "System".into(),
            None => "Launch app".into(),
        }
    }

    pub fn launch(&self) -> Result<(), String> {
        if let Some(keys) = &self.keys {
            actions::send_keys(keys)
        } else if let Some(action) = &self.action {
            actions::system(action)
        } else if !self.run.is_empty() {
            spawn(&self.run, &self.args).map_err(|e| e.to_string())
        } else {
            Err("nothing to do: set keys, action or run".into())
        }
    }
}

/// An output or input device button. `device` is matched against part of the Windows device name.
#[derive(Clone, Debug, Deserialize)]
pub struct DeviceConfig {
    pub label: String,
    pub device: String,
}

/// Uses `start` so a shortcut can be an exe, a file, a folder, a URL or a URI.
#[cfg(windows)]
pub fn spawn(target: &str, args: &[String]) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    std::process::Command::new("cmd")
        .args(["/C", "start", "", target])
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(drop)
}

#[cfg(not(windows))]
pub fn spawn(target: &str, args: &[String]) -> std::io::Result<()> {
    std::process::Command::new(target).args(args).spawn().map(drop)
}

/// Returns the config and the file it came from, if any.
pub fn load() -> (Config, Option<PathBuf>) {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    let candidates: Vec<PathBuf> = exe_dir
        .iter()
        .map(|dir| dir.join(FILE_NAME))
        .chain(std::iter::once(PathBuf::from(FILE_NAME)))
        .collect();

    if let Some(path) = candidates.into_iter().find(|p| p.is_file()) {
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        let config = read(&path).unwrap_or_else(|err| {
            eprintln!("config: {} has an error, using defaults:\n{err}", path.display());
            toml::from_str(DEFAULT_CONFIG).unwrap_or_default()
        });
        return (config, Some(path));
    }

    let mut written = None;
    if let Some(dir) = exe_dir {
        let path = dir.join(FILE_NAME);
        if std::fs::write(&path, DEFAULT_CONFIG).is_ok() {
            eprintln!("config: wrote a starter config to {}", path.display());
            written = Some(path);
        }
    }
    (toml::from_str(DEFAULT_CONFIG).unwrap_or_default(), written)
}

pub fn read(path: &Path) -> Result<Config, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    toml::from_str(&text).map_err(|e| e.to_string())
}
