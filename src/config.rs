//! Loads hub.toml. The file is looked up next to the executable first, then in the
//! current directory (handy for `cargo run`). If neither exists, a default copy is
//! written next to the executable so there is always something to edit.

use std::collections::BTreeMap;
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
    /// Drives on the storage card, such as ["C:", "D:"]. Empty shows the system drive.
    pub drives: Vec<String>,
    pub window: WindowConfig,
    pub sensors: SensorsConfig,
    pub hotkeys: HotkeysConfig,
    pub weather: WeatherConfig,
    pub network: NetworkConfig,
    pub calendar: CalendarConfig,
    pub notifications: NotificationsConfig,
    pub discord: DiscordConfig,
    /// Present when hub.toml has an [obs] section.
    pub obs: Option<ObsConfig>,
    /// Cards on the Monitor page for HWiNFO readings of your choice.
    #[serde(rename = "card")]
    pub cards: Vec<SensorCard>,
    #[serde(rename = "page")]
    pub pages: Vec<PageConfig>,
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
            drives: Vec::new(),
            window: WindowConfig::default(),
            sensors: SensorsConfig::default(),
            hotkeys: HotkeysConfig::default(),
            weather: WeatherConfig::default(),
            network: NetworkConfig::default(),
            calendar: CalendarConfig::default(),
            notifications: NotificationsConfig::default(),
            discord: DiscordConfig::default(),
            obs: None,
            cards: Vec::new(),
            pages: Vec::new(),
            shortcuts: Vec::new(),
            outputs: Vec::new(),
            inputs: Vec::new(),
        }
    }
}

/// Key combinations that show a page from any app, such as "Ctrl+Alt+1".
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct HotkeysConfig {
    pub music: Option<String>,
    pub monitor: Option<String>,
    pub shortcuts: Option<String>,
    pub widgets: Option<String>,
}

impl HotkeysConfig {
    /// Each combination with the page it shows (0 Music, 1 Monitor, 2 Shortcuts, 3 Widgets).
    pub fn bindings(&self) -> Vec<(String, i32)> {
        [&self.music, &self.monitor, &self.shortcuts, &self.widgets]
            .into_iter()
            .zip(0..)
            .filter_map(|(keys, page)| keys.clone().filter(|k| !k.trim().is_empty()).map(|k| (k, page)))
            .collect()
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

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct WindowConfig {
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub fullscreen: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct SensorsConfig {
    /// HWiNFO's name for the processor fan, for boards that don't label it "CPU".
    pub cpu_fan: Option<String>,
}

/// One button on the Shortcuts page. It does one of these, checked in this order: presses
/// `keys`, performs a built-in `action`, runs macro `steps`, runs a PowerShell `command`,
/// calls a web `url`, or opens `run`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ShortcutConfig {
    pub label: String,
    pub icon: Option<String>,
    /// Small text under the label; defaults to the key combination or "Launch app".
    pub hint: Option<String>,
    pub keys: Option<String>,
    pub action: Option<String>,
    /// Several steps run in order; see Step.
    pub steps: Vec<Step>,
    /// PowerShell to run when tapped; the first line it prints shows on the tile.
    pub command: Option<String>,
    /// A web address to call when tapped; the first line of the answer shows on the tile.
    pub url: Option<String>,
    /// GET, POST, PUT, PATCH or DELETE. Defaults to POST when there's a body, else GET.
    pub method: Option<String>,
    pub body: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub run: String,
    pub args: Vec<String>,
    /// Ask for a second tap before running. Defaults to on for shut down, restart and sign out.
    pub confirm: Option<bool>,
    /// For a command or web tile: also run it every this many seconds, and when the hub starts.
    pub refresh: Option<u32>,
    /// The shortcut page this tile is on. Tiles without one are on every page.
    pub page: Option<String>,
    /// What pressing and holding the tile (then letting go without dragging) does: one
    /// step, as in a macro.
    pub hold: Option<Step>,
    /// Makes the tile a folder: tapping it opens the tiles whose `in_folder` is this name.
    pub folder: Option<String>,
    /// Puts the tile inside the folder of this name instead of on the page.
    pub in_folder: Option<String>,
}

/// One step of a macro tile. Each step does one thing: presses `keys`, performs an
/// `action`, opens `run`, runs a `command`, calls a `url`, or waits `wait` seconds.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Step {
    pub keys: Option<String>,
    pub action: Option<String>,
    pub run: Option<String>,
    pub args: Vec<String>,
    pub command: Option<String>,
    pub url: Option<String>,
    pub method: Option<String>,
    pub body: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub wait: Option<f32>,
}

impl Step {
    pub fn web(&self) -> Option<WebRequest> {
        WebRequest::new(&self.url, &self.method, &self.body, &self.headers)
    }
}

/// A web request made by a tile or a macro step.
#[derive(Clone, Debug, PartialEq)]
pub struct WebRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

impl WebRequest {
    fn new(
        url: &Option<String>,
        method: &Option<String>,
        body: &Option<String>,
        headers: &BTreeMap<String, String>,
    ) -> Option<Self> {
        let url = url.as_deref().map(str::trim).filter(|u| !u.is_empty())?;
        let method = match method {
            Some(method) => method.trim().to_uppercase(),
            None if body.is_some() => "POST".into(),
            None => "GET".into(),
        };
        Some(Self {
            method,
            url: url.to_string(),
            headers: headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            body: body.clone(),
        })
    }
}

/// What a command, web or macro tile does, in the background; its result shows on the tile.
#[derive(Clone, Debug)]
pub enum Work {
    Command(String),
    Web(WebRequest),
    Steps(Vec<Step>),
}

/// Built-in actions that end the session or can't be undone; a stray tap shouldn't trigger them.
const RISKY_ACTIONS: [&str; 4] = ["shutdown", "restart", "signout", "empty-recycle-bin"];

impl ShortcutConfig {
    pub fn hint(&self) -> String {
        if let Some(hint) = &self.hint {
            return hint.clone();
        }
        if self.folder.is_some() {
            return "Folder".into();
        }
        if let Some(keys) = &self.keys {
            return keys.clone();
        }
        match self.action.as_deref() {
            Some("wifi") => "Wi-Fi on/off".into(),
            Some("bluetooth") => "Bluetooth on/off".into(),
            Some("night-light") => "Night light".into(),
            Some("dnd") => "Do not disturb".into(),
            Some("sleep" | "hibernate" | "shutdown" | "restart" | "signout") => "Power action".into(),
            Some("mic-mute") => "Mic on/off".into(),
            Some("mute") => "Sound on/off".into(),
            Some("dark-mode") => "Light or dark".into(),
            Some("power-saver" | "balanced" | "high-performance") => "Power plan".into(),
            Some("empty-recycle-bin") => "Recycle Bin".into(),
            Some(_) => "System".into(),
            None => match self.work() {
                Some(Work::Steps(steps)) => format!("{} steps", steps.len()),
                Some(Work::Web(_)) => "Web request".into(),
                Some(Work::Command(_)) => "Tap to run".into(),
                None => "Launch app".into(),
            },
        }
    }

    /// The background work, for command, web and macro tiles.
    pub fn work(&self) -> Option<Work> {
        if self.keys.is_some() || self.action.is_some() {
            return None;
        }
        if !self.steps.is_empty() {
            return Some(Work::Steps(self.steps.clone()));
        }
        if let Some(command) = self.command.as_deref().filter(|c| !c.trim().is_empty()) {
            return Some(Work::Command(command.to_string()));
        }
        WebRequest::new(&self.url, &self.method, &self.body, &self.headers).map(Work::Web)
    }

    /// How often a command or web tile runs by itself, in seconds (at least 5). Macros
    /// press keys and open things, so they only run when tapped.
    pub fn refresh_secs(&self) -> Option<u64> {
        if !matches!(self.work()?, Work::Command(_) | Work::Web(_)) {
            return None;
        }
        self.refresh.filter(|&s| s > 0).map(|s| u64::from(s.max(5)))
    }

    pub fn needs_confirm(&self) -> bool {
        self.confirm
            .unwrap_or_else(|| self.keys.is_none() && self.action.as_deref().is_some_and(|a| RISKY_ACTIONS.contains(&a)))
    }

    /// "mic" or "mute" for tiles that show whether the microphone or the sound is muted,
    /// "dark" for one that shows whether Windows is in dark mode.
    pub fn toggle(&self) -> &'static str {
        if self.keys.is_some() {
            return "";
        }
        match self.action.as_deref() {
            Some("mic-mute") => "mic",
            Some("mute") => "mute",
            Some("dark-mode") => "dark",
            Some("wifi") => "wifi",
            Some("bluetooth") => "bluetooth",
            Some("night-light") => "night-light",
            Some("dnd") => "dnd",
            _ => "",
        }
    }

    /// Whether this is a folder tile, or lives inside a folder rather than on the page.
    pub fn in_folder(&self) -> Option<&str> {
        self.in_folder.as_deref().map(str::trim).filter(|f| !f.is_empty())
    }

    /// The icon name, defaulting to a fitting one for the mute actions.
    pub fn icon_name(&self) -> Option<&str> {
        self.icon.as_deref().or(match self.toggle() {
            "mic" => Some("mic"),
            "mute" => Some("speaker"),
            "dark" => Some("moon"),
            "wifi" => Some("wifi"),
            "bluetooth" => Some("bluetooth"),
            "night-light" => Some("sun"),
            "dnd" => Some("bell"),
            _ if self.folder.is_some() => Some("folder"),
            _ => None,
        })
    }

    /// Whether this tile shows on the shortcut page called `page` (None: no pages in use).
    pub fn on_page(&self, page: Option<&str>) -> bool {
        match (page, self.page.as_deref()) {
            (Some(page), Some(own)) => own.eq_ignore_ascii_case(page),
            _ => true,
        }
    }

    /// Runs a keys, action or run tile. Command, web and macro tiles are `work()` instead.
    pub fn launch(&self) -> Result<(), String> {
        if let Some(keys) = &self.keys {
            actions::send_keys(keys)
        } else if let Some(action) = &self.action {
            actions::system(action)
        } else if !self.run.is_empty() {
            spawn(&self.run, &self.args).map_err(|e| e.to_string())
        } else {
            Err("nothing to do: set keys, action, steps, command, url or run".into())
        }
    }
}

/// A shortcut page, which can turn itself on while one of its apps is running.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct PageConfig {
    pub name: String,
    /// Program names such as "cs2.exe" or "steam".
    pub when_running: Vec<String>,
}

/// The network card's latency reading.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct NetworkConfig {
    /// The server whose answer time shows on the network card; empty hides it.
    pub latency_host: String,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            latency_host: "1.1.1.1".into(),
        }
    }
}

/// Calendars for the Widgets page: iCalendar (.ics) links, or files.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct CalendarConfig {
    pub urls: Vec<String>,
}

/// Which apps' notifications show on the Widgets page; empty shows every app's.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct NotificationsConfig {
    pub apps: Vec<String>,
}

/// Your own Discord app (from discord.com/developers), for the voice card.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct DiscordConfig {
    pub client_id: String,
    pub client_secret: String,
    /// One of the app's OAuth2 redirects; Discord wants it, though nothing is opened.
    pub redirect_uri: String,
}

impl Default for DiscordConfig {
    fn default() -> Self {
        Self {
            client_id: String::new(),
            client_secret: String::new(),
            redirect_uri: "http://localhost".into(),
        }
    }
}

impl DiscordConfig {
    pub fn is_set(&self) -> bool {
        !self.client_id.trim().is_empty() && !self.client_secret.trim().is_empty()
    }
}

/// OBS Studio's WebSocket server.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct ObsConfig {
    /// Set when hub.toml has an [obs] section; without one, OBS isn't contacted.
    #[serde(skip)]
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub password: String,
}

impl Default for ObsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            host: "localhost".into(),
            port: 4455,
            password: String::new(),
        }
    }
}

impl Config {
    /// [obs] from hub.toml, or a switched-off one without it.
    pub fn obs(&self) -> ObsConfig {
        match &self.obs {
            Some(obs) => ObsConfig { enabled: true, ..obs.clone() },
            None => ObsConfig::default(),
        }
    }
}

/// A card on the Monitor page for one HWiNFO reading, such as a water temperature.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct SensorCard {
    /// Shown on the card; defaults to the reading's name.
    pub label: String,
    /// Part of the sensor's name in HWiNFO, when several have a reading of that name.
    pub sensor: Option<String>,
    /// The reading's name in HWiNFO, e.g. "Water Temperature".
    pub reading: String,
    /// Draws a meter, full at this value.
    pub max: Option<f64>,
}

/// Where the weather in the top bar is for: a place name, or coordinates.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct WeatherConfig {
    pub place: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
}

impl WeatherConfig {
    pub fn is_set(&self) -> bool {
        self.place.as_deref().is_some_and(|p| !p.trim().is_empty()) || (self.latitude.is_some() && self.longitude.is_some())
    }
}

impl Config {
    /// The shortcut pages in order: those listed as [[page]], then any other names tiles
    /// use. Empty when no tile names a page.
    pub fn page_names(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        let listed = self.pages.iter().map(|p| p.name.as_str());
        let used = self.shortcuts.iter().filter_map(|s| s.page.as_deref());
        for name in listed.chain(used).map(str::trim).filter(|n| !n.is_empty()) {
            if !names.iter().any(|n| n.eq_ignore_ascii_case(name)) {
                names.push(name.to_string());
            }
        }
        if self.shortcuts.iter().any(|s| s.page.is_some()) {
            names
        } else {
            Vec::new()
        }
    }
}

/// An output or input device button. `device` is matched against part of the Windows device name.
#[derive(Clone, Debug, Deserialize, PartialEq)]
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

/// Moves the shortcut at `from` to `to` in hub.toml, keeping comments and blank lines.
pub fn move_shortcut(path: &Path, from: usize, to: usize) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let text = reorder_shortcuts(&text, from, to)?;
    std::fs::write(path, text).map_err(|e| e.to_string())
}

/// The text of hub.toml with one `[[shortcut]]` moved. Comments above a tile move with it,
/// except those above the first tile: they describe every shortcut, so they stay on top.
pub fn reorder_shortcuts(text: &str, from: usize, to: usize) -> Result<String, String> {
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| e.to_string())?;
    let shortcuts = doc
        .get_mut("shortcut")
        .and_then(toml_edit::Item::as_array_of_tables_mut)
        .ok_or("hub.toml has no [[shortcut]] entries")?;
    let count = shortcuts.len();
    if from >= count || to >= count {
        return Err(format!("no shortcut number {}", from.max(to) + 1));
    }

    let mut order: Vec<usize> = (0..count).collect();
    let moved = order.remove(from);
    order.insert(to, moved);

    let old: Vec<toml_edit::Table> = shortcuts.iter().cloned().collect();
    let mut tables: Vec<toml_edit::Table> = order.iter().map(|&i| old[i].clone()).collect();
    // Tables are written out sorted by position, so each slot keeps its original one.
    for (table, original) in tables.iter_mut().zip(&old) {
        table.set_position(original.position());
    }
    // Swap the leading comments between the old and new first tile.
    if order[0] != 0 {
        let top = old[0].decor().prefix().cloned();
        let new_first = old[order[0]].decor().prefix().cloned();
        let old_first = order.iter().position(|&i| i == 0).unwrap_or(0);
        set_prefix(&mut tables[0], top);
        set_prefix(&mut tables[old_first], new_first);
    }

    shortcuts.clear();
    for table in tables {
        shortcuts.push(table);
    }
    Ok(doc.to_string())
}

/// Adds a tile for a file, folder, app or internet shortcut at the end of hub.toml.
/// Returns its label.
pub fn add_shortcut_for(path: &Path, target: &Path) -> Result<String, String> {
    let shortcut = shortcut_for(target);
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let text = append_shortcut(&text, &shortcut)?;
    std::fs::write(path, text).map_err(|e| e.to_string())?;
    Ok(shortcut.label)
}

/// Adds a tile for a link dragged from a browser. Returns its label: the page's title, or
/// else the site's name.
pub fn add_link_shortcut(path: &Path, url: &str, title: Option<&str>) -> Result<String, String> {
    let site = url
        .split("://")
        .nth(1)
        .unwrap_or(url)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(url)
        .trim_start_matches("www.");
    let label: String = title.map(str::trim).filter(|t| !t.is_empty()).unwrap_or(site).chars().take(40).collect();
    let shortcut = ShortcutConfig {
        label,
        icon: Some("globe".into()),
        run: url.to_string(),
        ..Default::default()
    };
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let text = append_shortcut(&text, &shortcut)?;
    std::fs::write(path, text).map_err(|e| e.to_string())?;
    Ok(shortcut.label)
}

/// Adds a `[[card]]` for an HWiNFO reading to hub.toml.
pub fn add_sensor_card(path: &Path, card: &SensorCard) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| e.to_string())?;
    let mut table = toml_edit::Table::new();
    table.decor_mut().set_prefix("\n");
    table["label"] = toml_edit::value(card.label.as_str());
    if let Some(sensor) = &card.sensor {
        table["sensor"] = toml_edit::value(sensor.as_str());
    }
    table["reading"] = toml_edit::value(card.reading.as_str());
    match doc.get_mut("card") {
        Some(item) => item.as_array_of_tables_mut().ok_or("\"card\" in hub.toml isn't a list of [[card]] entries")?.push(table),
        None => {
            let mut cards = toml_edit::ArrayOfTables::new();
            cards.push(table);
            doc.insert("card", toml_edit::Item::ArrayOfTables(cards));
        }
    }
    std::fs::write(path, doc.to_string()).map_err(|e| e.to_string())
}

/// Deletes the `[[card]]` at `index` from hub.toml.
pub fn remove_sensor_card(path: &Path, index: usize) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| e.to_string())?;
    let cards = doc.get_mut("card").and_then(toml_edit::Item::as_array_of_tables_mut).ok_or("hub.toml has no cards")?;
    if index >= cards.len() {
        return Err(format!("no card number {}", index + 1));
    }
    cards.remove(index);
    std::fs::write(path, doc.to_string()).map_err(|e| e.to_string())
}

/// A tile that opens `target`, named after it, with an icon that fits.
pub fn shortcut_for(target: &Path) -> ShortcutConfig {
    let extension = target
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let icon = if target.is_dir() {
        Some("folder")
    } else {
        match extension.as_str() {
            "url" | "htm" | "html" => Some("globe"),
            "mp3" | "flac" | "wav" | "m4a" | "ogg" => Some("music"),
            "ps1" | "bat" | "cmd" => Some("terminal"),
            _ => None,
        }
    };
    // Drives have no file name: "D:\" becomes "D:".
    let label = target
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| target.display().to_string().trim_end_matches(['\\', '/']).to_string());
    ShortcutConfig {
        label,
        icon: icon.map(String::from),
        run: target.display().to_string(),
        ..Default::default()
    }
}

/// The text of hub.toml with a `[[shortcut]]` added after the last one.
pub fn append_shortcut(text: &str, shortcut: &ShortcutConfig) -> Result<String, String> {
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| e.to_string())?;
    let mut table = toml_edit::Table::new();
    table.decor_mut().set_prefix("\n");
    table["label"] = toml_edit::value(shortcut.label.as_str());
    if let Some(icon) = &shortcut.icon {
        table["icon"] = toml_edit::value(icon.as_str());
    }
    table["run"] = toml_edit::value(shortcut.run.as_str());

    match doc.get_mut("shortcut") {
        Some(item) => item
            .as_array_of_tables_mut()
            .ok_or("\"shortcut\" in hub.toml isn't a list of [[shortcut]] entries")?
            .push(table),
        None => {
            let mut shortcuts = toml_edit::ArrayOfTables::new();
            shortcuts.push(table);
            doc.insert("shortcut", toml_edit::Item::ArrayOfTables(shortcuts));
        }
    }
    Ok(doc.to_string())
}

/// What the tile editor on the hub sets.
#[derive(Clone, Debug, PartialEq)]
pub struct TileEdit {
    pub label: String,
    /// None: no icon set (a run tile then shows the app's own).
    pub icon: Option<String>,
    /// None: on every page.
    pub page: Option<String>,
    /// What the tile does. None leaves it as it is, for tiles the editor can't change
    /// (commands, web requests, macros).
    pub does: Option<TileAction>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TileAction {
    Keys(String),
    Action(String),
    /// An app, file, folder, website or URI.
    Open(String),
}

/// The fields that say what a tile does; giving it something else to do clears them all.
const DOES_FIELDS: [&str; 12] = [
    "keys", "action", "steps", "command", "url", "method", "body", "headers", "run", "args", "refresh", "folder",
];

/// What the tile editor shows for a tile: its action, or None for one it can't change.
pub fn tile_action(shortcut: &ShortcutConfig) -> Option<TileAction> {
    if let Some(keys) = &shortcut.keys {
        Some(TileAction::Keys(keys.clone()))
    } else if let Some(action) = &shortcut.action {
        Some(TileAction::Action(action.clone()))
    } else if shortcut.work().is_none() && shortcut.args.is_empty() && shortcut.folder.is_none() {
        Some(TileAction::Open(shortcut.run.clone()))
    } else {
        None
    }
}

/// Changes the shortcut at `index` in hub.toml, or adds one at the end with None.
pub fn save_tile(path: &Path, index: Option<usize>, edit: &TileEdit) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let text = with_tile(&text, index, edit)?;
    std::fs::write(path, text).map_err(|e| e.to_string())
}

/// The text of hub.toml with one shortcut changed or added. Fields the editor doesn't
/// know about (hint, confirm, comments) are kept.
pub fn with_tile(text: &str, index: Option<usize>, edit: &TileEdit) -> Result<String, String> {
    let label = edit.label.trim();
    if label.is_empty() {
        return Err("the tile needs a name".into());
    }
    if index.is_none() && edit.does.is_none() {
        return Err("choose what the new tile does".into());
    }
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| e.to_string())?;
    if doc.get("shortcut").is_none() {
        doc.insert("shortcut", toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()));
    }
    let shortcuts = doc
        .get_mut("shortcut")
        .and_then(toml_edit::Item::as_array_of_tables_mut)
        .ok_or("\"shortcut\" in hub.toml isn't a list of [[shortcut]] entries")?;
    let table = match index {
        Some(index) => shortcuts.get_mut(index).ok_or_else(|| format!("no shortcut number {}", index + 1))?,
        None => {
            let mut table = toml_edit::Table::new();
            table.decor_mut().set_prefix("\n");
            shortcuts.push(table);
            let last = shortcuts.len() - 1;
            shortcuts.get_mut(last).ok_or("could not add the shortcut")?
        }
    };

    table["label"] = toml_edit::value(label);
    let mut set_or_remove = |key: &str, value: &Option<String>| match value.as_deref().map(str::trim) {
        Some(value) if !value.is_empty() => table[key] = toml_edit::value(value),
        _ => {
            table.remove(key);
        }
    };
    set_or_remove("icon", &edit.icon);
    set_or_remove("page", &edit.page);
    if let Some(does) = &edit.does {
        for field in DOES_FIELDS {
            table.remove(field);
        }
        let (key, value) = match does {
            TileAction::Keys(keys) => ("keys", keys),
            TileAction::Action(action) => ("action", action),
            TileAction::Open(target) => ("run", target),
        };
        if value.trim().is_empty() {
            return Err(format!("the tile needs {}", match key {
                "keys" => "a key to press",
                "action" => "an action",
                _ => "something to open",
            }));
        }
        table[key] = toml_edit::value(value.trim());
    }
    Ok(doc.to_string())
}

/// Deletes the shortcut at `index` from hub.toml. Returns its label.
pub fn remove_shortcut(path: &Path, index: usize) -> Result<String, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let (text, label) = without_shortcut(&text, index)?;
    std::fs::write(path, text).map_err(|e| e.to_string())?;
    Ok(label)
}

/// The text of hub.toml without one `[[shortcut]]`, and that shortcut's label. The comments
/// above the first tile describe every shortcut, so they pass to the next one.
pub fn without_shortcut(text: &str, index: usize) -> Result<(String, String), String> {
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| e.to_string())?;
    let shortcuts = doc
        .get_mut("shortcut")
        .and_then(toml_edit::Item::as_array_of_tables_mut)
        .ok_or("hub.toml has no [[shortcut]] entries")?;
    if index >= shortcuts.len() {
        return Err(format!("no shortcut number {}", index + 1));
    }
    let removed = shortcuts.remove(index);
    if index == 0 {
        if let Some(next) = shortcuts.get_mut(0) {
            set_prefix(next, removed.decor().prefix().cloned());
        }
    }
    let label = removed.get("label").and_then(|v| v.as_str()).unwrap_or("tile").to_string();
    Ok((doc.to_string(), label))
}

fn set_prefix(table: &mut toml_edit::Table, prefix: Option<toml_edit::RawString>) {
    match prefix {
        Some(prefix) => table.decor_mut().set_prefix(prefix),
        None => table.decor_mut().clear(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
columns = 3

# About shortcuts
[[shortcut]]
label = \"A\"
run = \"a\"

# Bee's own note
[[shortcut]]
label = \"B\"
run = \"b\"

[[shortcut]]
label = \"C\"
run = \"c\"

[[output]]
label = \"Speakers\"
device = \"Realtek\"
";

    fn labels(text: &str) -> Vec<String> {
        let config: Config = toml::from_str(text).unwrap();
        config.shortcuts.into_iter().map(|s| s.label).collect()
    }

    #[test]
    fn reorder_moves_a_tile_and_keeps_its_comment() {
        let text = reorder_shortcuts(SAMPLE, 1, 2).unwrap();
        assert_eq!(labels(&text), ["A", "C", "B"]);
        let note = text.find("# Bee's own note").unwrap();
        assert!(note > text.find("label = \"C\"").unwrap());
        assert!(note < text.find("label = \"B\"").unwrap());
        // The other tables stay where they were.
        assert!(text.find("[[output]]").unwrap() > text.find("label = \"B\"").unwrap());
        assert!(text.starts_with("columns = 3\n\n# About shortcuts\n[[shortcut]]\nlabel = \"A\""));
    }

    #[test]
    fn reorder_keeps_the_top_comment_first() {
        let text = reorder_shortcuts(SAMPLE, 2, 0).unwrap();
        assert_eq!(labels(&text), ["C", "A", "B"]);
        let top = text.find("# About shortcuts").unwrap();
        assert!(top < text.find("label = \"C\"").unwrap());
        assert!(text.find("label = \"C\"").unwrap() < text.find("label = \"A\"").unwrap());
    }

    #[test]
    fn reorder_rejects_missing_tiles() {
        assert!(reorder_shortcuts(SAMPLE, 0, 3).is_err());
        assert!(reorder_shortcuts("columns = 3\n", 0, 0).is_err());
    }

    #[test]
    fn append_goes_after_the_last_tile() {
        let shortcut = ShortcutConfig {
            label: "Notes".into(),
            icon: Some("folder".into()),
            run: r"C:\Users\me\Notes".into(),
            ..Default::default()
        };
        let text = append_shortcut(SAMPLE, &shortcut).unwrap();
        assert_eq!(labels(&text), ["A", "B", "C", "Notes"]);
        let notes = text.find("label = \"Notes\"").unwrap();
        assert!(notes > text.find("label = \"C\"").unwrap());
        assert!(notes < text.find("[[output]]").unwrap());
        // Backslashes survive the round trip.
        let config: Config = toml::from_str(&text).unwrap();
        assert_eq!(config.shortcuts[3].run, r"C:\Users\me\Notes");

        let text = append_shortcut("columns = 3\n", &shortcut).unwrap();
        assert_eq!(labels(&text), ["Notes"]);
    }

    #[test]
    fn remove_keeps_the_top_comment() {
        let (text, label) = without_shortcut(SAMPLE, 0).unwrap();
        assert_eq!(label, "A");
        assert_eq!(labels(&text), ["B", "C"]);
        assert!(text.find("# About shortcuts").unwrap() < text.find("label = \"B\"").unwrap());
        let (text, _) = without_shortcut(SAMPLE, 1).unwrap();
        assert_eq!(labels(&text), ["A", "C"]);
        assert!(!text.contains("Bee's own note"));
        assert!(without_shortcut(SAMPLE, 3).is_err());
    }

    #[test]
    fn shortcuts_for_dropped_files() {
        let app = shortcut_for(Path::new(r"C:\Program Files\App\app.exe"));
        assert_eq!(app.label, "app");
        assert_eq!(app.icon, None);
        assert_eq!(app.run, r"C:\Program Files\App\app.exe");
        assert_eq!(shortcut_for(Path::new(r"C:\Links\News.url")).icon.as_deref(), Some("globe"));
        assert_eq!(shortcut_for(Path::new(r"C:\Links\Song.MP3")).icon.as_deref(), Some("music"));
        let folder = std::env::temp_dir();
        assert_eq!(shortcut_for(&folder).icon.as_deref(), Some("folder"));
    }

    #[test]
    fn editing_tiles() {
        let edit = TileEdit {
            label: "Bee".into(),
            icon: Some("game".into()),
            page: Some("Gaming".into()),
            does: Some(TileAction::Keys("Alt+F10".into())),
        };
        let text = with_tile(SAMPLE, Some(1), &edit).unwrap();
        let config: Config = toml::from_str(&text).unwrap();
        let bee = &config.shortcuts[1];
        assert_eq!((bee.label.as_str(), bee.keys.as_deref(), bee.icon.as_deref()), ("Bee", Some("Alt+F10"), Some("game")));
        assert_eq!(bee.page.as_deref(), Some("Gaming"));
        // The old run is gone, and its comment stays.
        assert!(bee.run.is_empty());
        assert!(text.contains("# Bee's own note"));

        // Only the name and icon change when what it does is left alone.
        let rename = TileEdit { label: "Sea".into(), icon: None, page: None, does: None };
        let text = with_tile(SAMPLE, Some(2), &rename).unwrap();
        let config: Config = toml::from_str(&text).unwrap();
        assert_eq!((config.shortcuts[2].label.as_str(), config.shortcuts[2].run.as_str()), ("Sea", "c"));

        // New tiles go at the end.
        let new = TileEdit {
            label: "Docs".into(),
            icon: None,
            page: None,
            does: Some(TileAction::Open("https://docs.rs".into())),
        };
        let text = with_tile(SAMPLE, None, &new).unwrap();
        assert_eq!(labels(&text), ["A", "B", "C", "Docs"]);

        assert!(with_tile(SAMPLE, None, &TileEdit { does: None, ..new.clone() }).is_err());
        assert!(with_tile(SAMPLE, Some(0), &TileEdit { label: " ".into(), ..new.clone() }).is_err());
        assert!(with_tile(SAMPLE, Some(9), &new).is_err());
    }

    #[test]
    fn tile_actions_for_the_editor() {
        let tile = |toml: &str| -> ShortcutConfig { toml::from_str(toml).unwrap() };
        assert_eq!(tile_action(&tile("keys = \"Ctrl+M\"")), Some(TileAction::Keys("Ctrl+M".into())));
        assert_eq!(tile_action(&tile("run = \"wt\"")), Some(TileAction::Open("wt".into())));
        assert_eq!(tile_action(&tile("command = \"Get-Date\"")), None);
        assert_eq!(tile_action(&tile("run = \"code\"\nargs = [\"-n\"]")), None);
    }

    #[test]
    fn macro_and_web_tiles() {
        let config: Config = toml::from_str(
            "[[shortcut]]\nlabel = \"Stream\"\n\
             steps = [{ keys = \"Ctrl+Shift+M\" }, { wait = 0.5 }, { run = \"obs64\" }]\n\
             [[shortcut]]\nlabel = \"Lamp\"\nurl = \"http://ha.local:8123/api/services/light/toggle\"\n\
             body = '{\"entity_id\": \"light.desk\"}'\nheaders = { Authorization = \"Bearer abc\" }\nrefresh = 30\n\
             [[shortcut]]\nlabel = \"Status\"\nurl = \"http://example.com/status\"\n",
        )
        .unwrap();
        let Some(Work::Steps(steps)) = config.shortcuts[0].work() else { panic!("expected steps") };
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[1].wait, Some(0.5));
        assert_eq!(config.shortcuts[0].hint(), "3 steps");
        // Macros only run when tapped.
        assert_eq!(config.shortcuts[0].refresh_secs(), None);

        let Some(Work::Web(lamp)) = config.shortcuts[1].work() else { panic!("expected a web request") };
        assert_eq!(lamp.method, "POST");
        assert_eq!(lamp.headers, [("Authorization".to_string(), "Bearer abc".to_string())]);
        assert_eq!(config.shortcuts[1].hint(), "Web request");
        assert_eq!(config.shortcuts[1].refresh_secs(), Some(30));
        let Some(Work::Web(status)) = config.shortcuts[2].work() else { panic!("expected a web request") };
        assert_eq!(status.method, "GET");
    }

    #[test]
    fn shortcut_pages() {
        let config: Config = toml::from_str(
            "[[page]]\nname = \"Gaming\"\nwhen_running = [\"cs2.exe\"]\n\
             [[shortcut]]\nlabel = \"Lock\"\naction = \"lock\"\n\
             [[shortcut]]\nlabel = \"Notes\"\nrun = \"notepad\"\npage = \"Work\"\n\
             [[shortcut]]\nlabel = \"Replay\"\nkeys = \"Alt+F10\"\npage = \"gaming\"\n",
        )
        .unwrap();
        assert_eq!(config.page_names(), ["Gaming", "Work"]);
        let on = |page: &str| -> Vec<&str> {
            config.shortcuts.iter().filter(|s| s.on_page(Some(page))).map(|s| s.label.as_str()).collect()
        };
        assert_eq!(on("Gaming"), ["Lock", "Replay"]);
        assert_eq!(on("Work"), ["Lock", "Notes"]);
        // Without any page on a tile, there are no pages.
        let plain: Config = toml::from_str("[[shortcut]]\nlabel = \"A\"\nrun = \"a\"\n").unwrap();
        assert!(plain.page_names().is_empty());
    }

    #[test]
    fn command_tiles_and_hotkeys() {
        let config: Config = toml::from_str(
            "drives = [\"C:\", \"D:\"]\n\
             [hotkeys]\nmusic = \"Ctrl+Alt+1\"\nshortcuts = \"Ctrl+Alt+3\"\n\
             [[shortcut]]\nlabel = \"Ping\"\ncommand = \"Test-Connection 1.1.1.1 -Count 1\"\nrefresh = 2\n\
             [[shortcut]]\nlabel = \"Both\"\nkeys = \"Ctrl+M\"\ncommand = \"ignored\"\n",
        )
        .unwrap();
        assert_eq!(config.drives, ["C:", "D:"]);
        assert_eq!(
            config.hotkeys.bindings(),
            [("Ctrl+Alt+1".to_string(), 0), ("Ctrl+Alt+3".to_string(), 2)]
        );
        let ping = &config.shortcuts[0];
        assert!(matches!(ping.work(), Some(Work::Command(c)) if c == "Test-Connection 1.1.1.1 -Count 1"));
        assert_eq!(ping.refresh_secs(), Some(5));
        assert_eq!(ping.hint(), "Tap to run");
        // Keys come first, so the command never runs.
        assert!(config.shortcuts[1].work().is_none());
        assert_eq!(config.shortcuts[1].refresh_secs(), None);
    }

    #[test]
    fn new_sections_and_tile_fields() {
        let config: Config = toml::from_str(
            "[calendar]\nurls = [\"https://example.com/a.ics\"]\n\
             [obs]\npassword = \"pw\"\n\
             [discord]\nclient_id = \"1\"\nclient_secret = \"s\"\n\
             [[card]]\nlabel = \"Water\"\nreading = \"Water Temperature\"\n\
             [[shortcut]]\nlabel = \"Games\"\nfolder = \"Games\"\n\
             [[shortcut]]\nlabel = \"Steam\"\nrun = \"steam:\"\nin_folder = \"Games\"\nhold = { keys = \"Alt+F10\" }\n\
             [[shortcut]]\nlabel = \"Wi-Fi\"\naction = \"wifi\"\n",
        )
        .unwrap();
        assert_eq!(config.calendar.urls.len(), 1);
        let obs = config.obs();
        assert!(obs.enabled);
        assert_eq!((obs.port, obs.password.as_str()), (4455, "pw"));
        assert!(!Config::default().obs().enabled);
        assert!(config.discord.is_set());
        assert_eq!(config.discord.redirect_uri, "http://localhost");
        assert_eq!(config.cards[0].reading, "Water Temperature");
        assert_eq!(config.shortcuts[0].hint(), "Folder");
        assert_eq!(config.shortcuts[0].icon_name(), Some("folder"));
        assert_eq!(config.shortcuts[1].in_folder(), Some("Games"));
        assert_eq!(config.shortcuts[1].hold.as_ref().and_then(|h| h.keys.as_deref()), Some("Alt+F10"));
        assert_eq!(config.shortcuts[2].toggle(), "wifi");
    }

    #[test]
    fn sensor_cards_and_links_in_hub_toml() {
        let path = std::env::temp_dir().join(format!("pc-hub-cards-{}.toml", std::process::id()));
        std::fs::write(&path, SAMPLE).unwrap();
        let card = SensorCard { label: "Pump".into(), sensor: Some("Aquacomputer".into()), reading: "Pump Speed".into(), max: None };
        add_sensor_card(&path, &card).unwrap();
        add_sensor_card(&path, &SensorCard { label: "Flow".into(), reading: "Flow".into(), ..Default::default() }).unwrap();
        let config = read(&path).unwrap();
        assert_eq!(config.cards.len(), 2);
        assert_eq!(config.cards[0].sensor.as_deref(), Some("Aquacomputer"));
        remove_sensor_card(&path, 0).unwrap();
        assert_eq!(read(&path).unwrap().cards[0].label, "Flow");

        assert_eq!(add_link_shortcut(&path, "https://www.rust-lang.org/learn", None).unwrap(), "rust-lang.org");
        assert_eq!(add_link_shortcut(&path, "https://docs.rs", Some("Docs.rs")).unwrap(), "Docs.rs");
        let config = read(&path).unwrap();
        assert_eq!(config.shortcuts.last().map(|s| s.run.as_str()), Some("https://docs.rs"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn default_config_parses() {
        let config: Config = toml::from_str(DEFAULT_CONFIG).unwrap();
        assert!(!config.shortcuts.is_empty());
        assert!(config.window.fullscreen);
    }

    #[test]
    fn colors() {
        assert_eq!(parse_color("#5EEAD4"), Some(slint::Color::from_rgb_u8(0x5E, 0xEA, 0xD4)));
        assert_eq!(parse_color(" 00ff00 "), Some(slint::Color::from_rgb_u8(0, 255, 0)));
        assert_eq!(parse_color("#FFF"), None);
        assert_eq!(parse_color("#GGGGGG"), None);
    }

    #[test]
    fn hints_and_confirmation() {
        let action = |name: &str| ShortcutConfig {
            action: Some(name.into()),
            ..Default::default()
        };
        assert_eq!(action("shutdown").hint(), "Power action");
        assert_eq!(action("mic-mute").hint(), "Mic on/off");
        assert!(action("restart").needs_confirm());
        assert!(!action("lock").needs_confirm());
        assert!(!ShortcutConfig { confirm: Some(false), ..action("signout") }.needs_confirm());
        assert_eq!(action("mic-mute").toggle(), "mic");
        assert_eq!(action("mic-mute").icon_name(), Some("mic"));
        let keys = ShortcutConfig { keys: Some("Ctrl+M".into()), ..Default::default() };
        assert_eq!(keys.hint(), "Ctrl+M");
        assert_eq!(keys.toggle(), "");
    }
}
