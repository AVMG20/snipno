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
    /// Where "capture and upload" sends screenshots.
    pub upload: Upload,
}

impl Default for Config {
    fn default() -> Self {
        Self { hotkey: DEFAULT_HOTKEY.to_string(), tool_keys: ToolKeys::default(), upload: Upload::default() }
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
}

impl Default for ToolKeys {
    fn default() -> Self {
        Self {
            select: "S".into(),
            pen: "P".into(),
            arrow: "A".into(),
            rect: "R".into(),
            ellipse: "E".into(),
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
        }
    }

    pub fn set(&mut self, tool: Tool, key: String) {
        match tool {
            Tool::Select => self.select = key,
            Tool::Pen => self.pen = key,
            Tool::Arrow => self.arrow = key,
            Tool::Rect => self.rect = key,
            Tool::Ellipse => self.ellipse = key,
        }
    }
}

/// A ShareX-style custom uploader: the screenshot is POSTed as multipart form data (field
/// `file`), and the link is read from the JSON response.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Upload {
    /// Global hotkey for capture-and-upload; empty means none.
    pub hotkey: String,
    /// Endpoint, e.g. `https://img.example.com/api/images`. Empty disables uploading.
    pub url: String,
    /// Sent as `Authorization: Bearer <token>` when set.
    pub token: String,
    /// Comma-separated labels, sent as the `tags` form field. Editable per capture.
    pub labels: String,
    /// Extra form fields, `key=value&key=value` (e.g. `isPublic=true`).
    pub fields: String,
    /// Link template with `{json:path}` placeholders, e.g.
    /// `https://img.example.com/i/{json:images[0].slug}`. Empty: use the response's `url`/`link`.
    pub link: String,
    /// File name the image is uploaded as, `.png` added; `{date}`, `{time}` and `{timestamp}`
    /// are filled in. Empty means `{date}-{time}`.
    pub name: String,
}

pub const DEFAULT_UPLOAD_NAME: &str = "screenshot-{date}-{time}";

impl Default for Upload {
    fn default() -> Self {
        Self {
            hotkey: String::new(),
            url: String::new(),
            token: String::new(),
            labels: String::new(),
            fields: String::new(),
            link: String::new(),
            name: DEFAULT_UPLOAD_NAME.into(),
        }
    }
}

impl Upload {
    pub fn enabled(&self) -> bool {
        !self.url.trim().is_empty()
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

pub fn save(config: &Config) -> io::Result<()> {
    let path = path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(config).map_err(io::Error::other)?;
    fs::write(path, json)
}
