use crate::annotate::Tool;
use serde::{Deserialize, Serialize};
use std::{fs, io, path::PathBuf};

/// ⌘⇧2 — next to the built-in ⌘⇧3/4/5 screenshot shortcuts, which macOS keeps for itself.
pub const DEFAULT_HOTKEY: &str = "cmd+shift+2";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Config {
    /// Hotkey in `global-hotkey` string form, e.g. `cmd+shift+2`.
    pub hotkey: String,
    /// Keys that pick a tool inside the capture overlay.
    pub tool_keys: ToolKeys,
    /// Keeping a copy of every capture in a folder.
    pub save: Save,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: DEFAULT_HOTKEY.to_string(),
            tool_keys: ToolKeys::default(),
            save: Save::default(),
        }
    }
}

/// Every capture is also written to a folder as a PNG, and listed under "Recent screenshots" in
/// the menu bar menu. Only files snipno saved itself are ever cleaned up.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Save {
    pub enabled: bool,
    /// Folder to save in; `~` is your home folder. Empty means [`default_folder`].
    pub folder: String,
    /// File name template: `{date}`, `{time}` and `{timestamp}` are filled in, `.png` added.
    pub name: String,
    /// How many saved screenshots to keep; older ones are deleted. 0 keeps them all.
    pub keep: u32,
}

pub const DEFAULT_SAVE_NAME: &str = "Screenshot {date} at {time}";
pub const DEFAULT_KEEP: u32 = 200;

impl Default for Save {
    fn default() -> Self {
        Self { enabled: true, folder: String::new(), name: DEFAULT_SAVE_NAME.into(), keep: DEFAULT_KEEP }
    }
}

impl Save {
    /// The folder screenshots go to, with `~` expanded.
    pub fn folder_path(&self) -> PathBuf {
        let folder = self.folder.trim();
        if folder.is_empty() {
            return default_folder();
        }
        let home = dirs::home_dir().unwrap_or_default();
        match folder.strip_prefix('~') {
            Some("") => home,
            Some(rest) if rest.starts_with('/') => home.join(&rest[1..]),
            _ if PathBuf::from(folder).is_absolute() => PathBuf::from(folder),
            _ => home.join(folder),
        }
    }
}

/// `~/Pictures/snipno`.
pub fn default_folder() -> PathBuf {
    dirs::picture_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Pictures")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("snipno")
}

/// `path` with the home folder shown as `~`.
pub fn tilde(path: &std::path::Path) -> String {
    let s = path.display().to_string();
    match dirs::home_dir() {
        Some(home) if path.starts_with(&home) => {
            format!("~{}", s.strip_prefix(&home.display().to_string()).unwrap_or(&s))
        }
        _ => s,
    }
}

/// Overlay tool shortcuts, same string form as the hotkey (`P`, `cmd+2`, …). Plain letters by
/// default: they need no modifier, so they work on every keyboard and with any modifier remap.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct ToolKeys {
    pub select: String,
    pub pen: String,
    pub arrow: String,
    pub rect: String,
    pub ellipse: String,
    pub pixelate: String,
    pub number: String,
}

impl Default for ToolKeys {
    fn default() -> Self {
        Self {
            select: "S".into(),
            pen: "P".into(),
            arrow: "A".into(),
            rect: "R".into(),
            ellipse: "E".into(),
            pixelate: "B".into(),
            number: "N".into(),
        }
    }
}

impl ToolKeys {
    pub fn get(&self, tool: Tool) -> &str {
        match tool {
            Tool::Select => &self.select,
            Tool::Pen => &self.pen,
            Tool::Arrow => &self.arrow,
            Tool::Rect => &self.rect,
            Tool::Ellipse => &self.ellipse,
            Tool::Pixelate => &self.pixelate,
            Tool::Number => &self.number,
        }
    }

    pub fn set(&mut self, tool: Tool, key: String) {
        match tool {
            Tool::Select => self.select = key,
            Tool::Pen => self.pen = key,
            Tool::Arrow => self.arrow = key,
            Tool::Rect => self.rect = key,
            Tool::Ellipse => self.ellipse = key,
            Tool::Pixelate => self.pixelate = key,
            Tool::Number => self.number = key,
        }
    }
}

pub fn path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("snipno")
        .join("config.json")
}

/// Missing or unreadable config falls back to defaults.
pub fn load() -> Config {
    fs::read_to_string(path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Older versions stored upload settings, including an API token in plain text. Rewrite the
/// config without them; called once when the daemon starts.
pub fn drop_old_settings() {
    let Ok(text) = fs::read_to_string(path()) else { return };
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str(&text) else { return };
    if map.contains_key("upload") {
        if let Ok(config) = serde_json::from_str::<Config>(&text) {
            let _ = save(&config);
        }
    }
}

pub fn save(config: &Config) -> io::Result<()> {
    let path = path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(config).map_err(io::Error::other)?;
    fs::write(path, json)
}
