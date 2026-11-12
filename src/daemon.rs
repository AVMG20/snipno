//! Background process: menu bar icon + global hotkey. The overlay and settings windows run as
//! child processes (`snipno capture`, `snipno settings`) so each gets a fresh eframe event loop
//! and the daemon never has to hide/show windows.
//!
//! The settings child talks back over stdout, one message per line (see [`SettingsMsg`]), so
//! the hotkey is only paused while a new one is actually being recorded and a saved hotkey takes
//! effect immediately — not when the window closes.

use crate::{config, hotkey, icon, upload};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use std::{
    error::Error,
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};
use tao::{
    event::{Event, StartCause},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
};
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu},
};

/// How often the daemon checks whether an upload added a link to the history.
const HISTORY_POLL: Duration = Duration::from_secs(2);

/// Lines the settings process prints to stdout.
pub const MSG_PAUSE: &str = "snipno:pause-hotkey";
pub const MSG_RESUME: &str = "snipno:resume-hotkey";
pub const MSG_RELOAD: &str = "snipno:reload-config";

#[derive(Debug, PartialEq)]
enum SettingsMsg {
    Pause,
    Resume,
    Reload,
    Closed,
}

impl SettingsMsg {
    fn parse(line: &str) -> Option<Self> {
        match line.trim() {
            MSG_PAUSE => Some(Self::Pause),
            MSG_RESUME => Some(Self::Resume),
            MSG_RELOAD => Some(Self::Reload),
            _ => None,
        }
    }
}

enum UserEvent {
    HotKey(GlobalHotKeyEvent),
    Menu(MenuEvent),
    /// Message from settings process number `.0`; stale generations are ignored.
    Settings(u32, SettingsMsg),
}

struct Hotkeys {
    manager: GlobalHotKeyManager,
    /// Capture to the clipboard.
    active: Option<HotKey>,
    /// Capture and upload; only registered once an upload URL is set.
    upload: Option<HotKey>,
}

impl Hotkeys {
    fn pause(&mut self) {
        for hk in [self.active.take(), self.upload.take()].into_iter().flatten() {
            let _ = self.manager.unregister(hk);
        }
    }

    fn register(&mut self, shortcut: &str) -> Result<HotKey, String> {
        let hk = hotkey::parse(shortcut).map_err(|e| format!("invalid hotkey {shortcut:?}: {e}"))?;
        self.manager
            .register(hk)
            .map(|()| hk)
            .map_err(|e| format!("could not register {}: {e}", hotkey::pretty(shortcut)))
    }

    /// (Re)register both hotkeys from the config file and update their menu items.
    fn apply_config(&mut self, capture_item: &MenuItem, upload_item: &MenuItem) {
        self.pause();
        let cfg = config::load();
        capture_item.set_text(match self.register(&cfg.hotkey) {
            Ok(hk) => {
                self.active = Some(hk);
                format!("Take screenshot    {}", hotkey::pretty(&cfg.hotkey))
            }
            Err(e) => {
                eprintln!("snipno: {e}");
                "Take screenshot    (hotkey unavailable)".to_string()
            }
        });

        let up = &cfg.upload;
        upload_item.set_enabled(up.enabled());
        upload_item.set_text(if !up.enabled() {
            "Screenshot & upload    (set up in Settings)".to_string()
        } else if up.hotkey.is_empty() {
            "Screenshot & upload".to_string()
        } else {
            match self.register(&up.hotkey) {
                Ok(hk) => {
                    self.upload = Some(hk);
                    format!("Screenshot & upload    {}", hotkey::pretty(&up.hotkey))
                }
                Err(e) => {
                    eprintln!("snipno: {e}");
                    "Screenshot & upload    (hotkey unavailable)".to_string()
                }
            }
        });
    }
}

/// "Copy recent link" submenu: the last few uploaded links, click one to copy it again. The
/// upload process writes the history file; the daemon picks up changes by its modified time.
struct Recent {
    menu: Submenu,
    items: Vec<(MenuItem, String)>,
    seen: Option<SystemTime>,
}

impl Recent {
    fn new() -> Self {
        let mut recent = Self { menu: Submenu::new("Copy recent link", false), items: Vec::new(), seen: None };
        recent.refresh();
        recent
    }

    fn refresh(&mut self) {
        let modified = std::fs::metadata(upload::history_path()).and_then(|m| m.modified()).ok();
        if modified == self.seen {
            return;
        }
        self.seen = modified;
        for (item, _) in self.items.drain(..) {
            let _ = self.menu.remove(&item);
        }
        for link in upload::history() {
            let item = MenuItem::new(menu_label(&link), true, None);
            if self.menu.append(&item).is_ok() {
                self.items.push((item, link));
            }
        }
        self.menu.set_enabled(!self.items.is_empty());
    }

    /// Copy the link behind a clicked menu item; false if the item isn't one of ours.
    fn copy(&self, id: &tray_icon::menu::MenuId) -> bool {
        let Some((_, link)) = self.items.iter().find(|(item, _)| item.id() == id) else { return false };
        if let Err(e) = arboard::Clipboard::new().and_then(|mut cb| cb.set_text(link.clone())) {
            eprintln!("snipno: copying the link failed: {e}");
        }
        true
    }
}

/// A link as a menu item: without the scheme, long ones cut in the middle so the end (usually
/// the image's id) stays visible.
fn menu_label(link: &str) -> String {
    const MAX: usize = 48;
    let s = link.trim_start_matches("https://").trim_start_matches("http://");
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= MAX {
        return s.to_string();
    }
    let head: String = chars[..MAX / 2].iter().collect();
    let tail: String = chars[chars.len() - (MAX / 2 - 1)..].iter().collect();
    format!("{head}…{tail}")
}

pub fn run() -> Result<(), Box<dyn Error>> {
    #[allow(unused_mut)]
    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        // Menu bar only: no Dock icon, no app menu.
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
    }

    let proxy = event_loop.create_proxy();
    GlobalHotKeyEvent::set_event_handler(Some(move |e| {
        let _ = proxy.send_event(UserEvent::HotKey(e));
    }));
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |e| {
        let _ = proxy.send_event(UserEvent::Menu(e));
    }));

    let mut hotkeys = Hotkeys { manager: GlobalHotKeyManager::new()?, active: None, upload: None };
    let capture_item = MenuItem::new("Take screenshot", true, None);
    let upload_item = MenuItem::new("Screenshot & upload", false, None);
    hotkeys.apply_config(&capture_item, &upload_item);
    let mut recent = Recent::new();
    let settings_item = MenuItem::new("Settings…", true, None);
    let quit_item = MenuItem::new("Quit snipno", true, None);
    let menu = Menu::new();
    menu.append_items(&[
        &capture_item,
        &upload_item,
        &recent.menu,
        &settings_item,
        &PredefinedMenuItem::separator(),
        &quit_item,
    ])?;

    let mut tray: Option<TrayIcon> = None;
    let mut menu = Some(menu);
    let capturing = Arc::new(AtomicBool::new(false));
    let mut settings: Option<(u32, Child)> = None;
    let mut generation = 0u32;
    let proxy = event_loop.create_proxy();

    event_loop.run(move |event, _, control_flow| {
        // Wake up now and then to pick up links added by finished uploads.
        *control_flow = ControlFlow::WaitUntil(Instant::now() + HISTORY_POLL);
        match event {
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => recent.refresh(),

            // macOS: the status item must be created once the event loop is running.
            Event::NewEvents(StartCause::Init) => {
                let tray_px = icon::tray_icon(44);
                let built = Icon::from_rgba(icon::to_rgba(&tray_px), 44, 44)
                    .map_err(|e| e.to_string())
                    .and_then(|icon| {
                        TrayIconBuilder::new()
                            .with_menu(Box::new(menu.take().expect("menu used once")))
                            .with_icon_templated(icon)
                            .with_tooltip("snipno")
                            .build()
                            .map_err(|e| e.to_string())
                    });
                match built {
                    Ok(t) => tray = Some(t),
                    Err(e) => eprintln!("snipno: could not create menu bar icon: {e}"),
                }
            }

            Event::UserEvent(UserEvent::HotKey(e)) => {
                if e.state() != HotKeyState::Pressed {
                    return;
                }
                if hotkeys.active.is_some_and(|hk| hk.id() == e.id()) {
                    launch_capture(&capturing, false);
                } else if hotkeys.upload.is_some_and(|hk| hk.id() == e.id()) {
                    launch_capture(&capturing, true);
                }
            }

            Event::UserEvent(UserEvent::Menu(e)) => {
                if recent.copy(&e.id) {
                    // Copied a recent link.
                } else if e.id == *capture_item.id() {
                    launch_capture(&capturing, false);
                } else if e.id == *upload_item.id() {
                    launch_capture(&capturing, true);
                } else if e.id == *settings_item.id() {
                    // Already open (maybe lost behind other windows)? Replace it with a fresh one
                    // in front. Settings save immediately, so nothing is lost.
                    if let Some((_, mut old)) = settings.take() {
                        let _ = old.kill();
                        let _ = old.wait();
                        hotkeys.apply_config(&capture_item, &upload_item);
                    }
                    generation += 1;
                    match open_settings(generation, proxy.clone()) {
                        Ok(child) => settings = Some((generation, child)),
                        Err(e) => eprintln!("snipno: could not open settings: {e}"),
                    }
                } else if e.id == *quit_item.id() {
                    if let Some((_, mut child)) = settings.take() {
                        let _ = child.kill();
                    }
                    tray.take();
                    *control_flow = ControlFlow::Exit;
                }
            }

            Event::UserEvent(UserEvent::Settings(id, msg)) => {
                let current = settings.as_ref().is_some_and(|(g, _)| *g == id);
                if !current {
                    return; // a replaced settings window finishing up
                }
                match msg {
                    SettingsMsg::Pause => hotkeys.pause(),
                    SettingsMsg::Resume | SettingsMsg::Reload => hotkeys.apply_config(&capture_item, &upload_item),
                    SettingsMsg::Closed => {
                        if let Some((_, mut child)) = settings.take() {
                            let _ = child.wait();
                        }
                        // Whatever happened in there, never leave the hotkey switched off.
                        hotkeys.apply_config(&capture_item, &upload_item);
                    }
                }
            }

            _ => {}
        }
    });
}

/// Spawn `snipno settings` and forward its stdout messages to the event loop.
fn open_settings(id: u32, proxy: EventLoopProxy<UserEvent>) -> std::io::Result<Child> {
    let mut child = Command::new(std::env::current_exe()?)
        .arg("settings")
        .stdout(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().expect("stdout is piped");
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(msg) = SettingsMsg::parse(&line) {
                let _ = proxy.send_event(UserEvent::Settings(id, msg));
            }
        }
        // EOF: the settings process exited (or was killed).
        let _ = proxy.send_event(UserEvent::Settings(id, SettingsMsg::Closed));
    });
    Ok(child)
}

/// Open an overlay on every display: one `snipno capture --monitor <id>` process each. The
/// display under the cursor is started last so it ends up with keyboard focus. As soon as one
/// overlay finishes (copied or cancelled, exit code 0) the others are closed.
/// With `upload`, the overlays upload the selection and copy its link instead.
fn launch_capture(capturing: &Arc<AtomicBool>, upload: bool) {
    // Ignore the hotkey while overlays are already up.
    if capturing.swap(true, Ordering::SeqCst) {
        return;
    }
    let exe = std::env::current_exe().unwrap_or_default();
    let mut children: Vec<Child> = monitor_ids_cursor_last()
        .into_iter()
        .filter_map(|id| {
            let mut cmd = Command::new(&exe);
            cmd.arg("capture");
            if let Some(id) = id {
                cmd.args(["--monitor", &id.to_string()]);
            }
            if upload {
                cmd.arg("--upload");
            }
            cmd.spawn().map_err(|e| eprintln!("snipno: could not start capture: {e}")).ok()
        })
        .collect();

    let capturing = capturing.clone();
    std::thread::spawn(move || {
        'watch: while !children.is_empty() {
            let mut i = 0;
            while i < children.len() {
                match children[i].try_wait() {
                    Ok(None) => i += 1,
                    Ok(Some(status)) if status.success() => break 'watch,
                    // An overlay that failed to start (e.g. a display that can't be captured)
                    // just drops out; the others stay up.
                    _ => {
                        let mut failed = children.remove(i);
                        let _ = failed.kill();
                        let _ = failed.wait();
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        for child in &mut children {
            let _ = child.kill();
            let _ = child.wait();
        }
        capturing.store(false, Ordering::SeqCst);
    });
}

/// Display ids with the one under the cursor last. `[None]` (single overlay on the cursor's
/// display) if displays can't be listed.
fn monitor_ids_cursor_last() -> Vec<Option<u32>> {
    let Ok(monitors) = xcap::Monitor::all() else { return vec![None] };
    let cursor_id = crate::macos::cursor_position()
        .and_then(|(x, y)| xcap::Monitor::from_point(x as i32, y as i32).ok())
        .and_then(|m| m.id().ok());
    let mut ids: Vec<u32> = monitors.iter().filter_map(|m| m.id().ok()).collect();
    if ids.is_empty() {
        return vec![None];
    }
    if let Some(pos) = ids.iter().position(|id| Some(*id) == cursor_id) {
        let id = ids.remove(pos);
        ids.push(id);
    }
    ids.into_iter().map(Some).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_settings_messages() {
        assert_eq!(SettingsMsg::parse("snipno:pause-hotkey\n"), Some(SettingsMsg::Pause));
        assert_eq!(SettingsMsg::parse(MSG_RESUME), Some(SettingsMsg::Resume));
        assert_eq!(SettingsMsg::parse(MSG_RELOAD), Some(SettingsMsg::Reload));
        assert_eq!(SettingsMsg::parse("snipno: snapshot saved"), None);
    }

    #[test]
    fn menu_labels_drop_the_scheme_and_keep_the_end_of_long_links() {
        assert_eq!(menu_label("https://img.example.com/i/abc"), "img.example.com/i/abc");
        let long = format!("https://img.example.com/{}/xyz123", "a".repeat(60));
        let label = menu_label(&long);
        assert_eq!(label.chars().count(), 48);
        assert!(label.starts_with("img.example.com/") && label.ends_with("/xyz123"), "{label}");
    }
}
