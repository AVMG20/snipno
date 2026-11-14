//! Background process: menu bar icon + global hotkey. The overlay and settings windows run as
//! child processes (`snipno capture`, `snipno settings`) so each gets a fresh eframe event loop
//! and the daemon never has to hide/show windows.
//!
//! The settings child talks back over stdout, one message per line (see [`SettingsMsg`]), so
//! the hotkey is only paused while a new one is actually being recorded and a saved hotkey takes
//! effect immediately — not when the window closes.

use crate::{config, hotkey, icon, store};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use std::{
    error::Error,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
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
    menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu},
};

/// How often the daemon checks whether a capture changed the history.
const HISTORY_POLL: Duration = Duration::from_secs(2);

/// Lines the settings process prints to stdout.
pub const MSG_PAUSE: &str = "snipno:pause-hotkey";
pub const MSG_RESUME: &str = "snipno:resume-hotkey";
pub const MSG_RELOAD: &str = "snipno:reload-config";
/// Printed by a capture process once its overlay has closed (copied or cancelled); it may still
/// be saving the screenshot.
pub const MSG_CAPTURE_DONE: &str = "snipno:capture-done";

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
    active: Option<HotKey>,
}

impl Hotkeys {
    fn pause(&mut self) {
        if let Some(hk) = self.active.take() {
            let _ = self.manager.unregister(hk);
        }
    }

    /// (Re)register the hotkey from the config file and update its menu item.
    fn apply_config(&mut self, capture_item: &MenuItem) {
        self.pause();
        let shortcut = config::load().hotkey;
        let registered = hotkey::parse(&shortcut)
            .map_err(|e| format!("invalid hotkey {shortcut:?}: {e}"))
            .and_then(|hk| {
                self.manager
                    .register(hk)
                    .map(|()| hk)
                    .map_err(|e| format!("could not register {}: {e}", hotkey::pretty(&shortcut)))
            });
        capture_item.set_text(match registered {
            Ok(hk) => {
                self.active = Some(hk);
                format!("Take screenshot    {}", hotkey::pretty(&shortcut))
            }
            Err(e) => {
                eprintln!("snipno: {e}");
                "Take screenshot    (hotkey unavailable)".to_string()
            }
        });
    }
}

/// How many captures "Recent screenshots" lists.
const RECENT_SHOWN: usize = 10;

enum RecentAction {
    CopyImage(PathBuf),
    Reveal(PathBuf),
    OpenFolder,
}

/// "Recent screenshots" submenu: the last few saved captures, each with a submenu to copy the
/// image again or show it in Finder. Captures write the history file; the daemon picks up changes
/// by its modified time.
struct Recent {
    menu: Submenu,
    actions: Vec<(MenuId, RecentAction)>,
    seen: Option<SystemTime>,
    /// Files the menu offers, to notice when one is deleted or moved in Finder.
    shown: Vec<PathBuf>,
}

impl Recent {
    fn new() -> Self {
        let mut recent = Self { menu: Submenu::new("Recent screenshots", false), actions: Vec::new(), seen: None, shown: Vec::new() };
        recent.refresh(true);
        recent
    }

    /// Rebuild the submenu if the history changed or a listed file is gone, or always with
    /// `force` (settings changed).
    fn refresh(&mut self, force: bool) {
        let modified = std::fs::metadata(store::history_path()).and_then(|m| m.modified()).ok();
        if !force && modified == self.seen && self.shown.iter().all(|f| f.is_file()) {
            return;
        }
        self.seen = modified;
        while self.menu.remove_at(0).is_some() {}
        self.actions.clear();

        let cfg = config::load();
        let entries: Vec<PathBuf> = store::history()
            .into_iter()
            .filter_map(|e| e.existing_file().map(Path::to_path_buf))
            .take(RECENT_SHOWN)
            .collect();
        for file in &entries {
            let name = file.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let sub = Submenu::new(name, true);
            add(&mut self.actions, &sub, "Copy image", RecentAction::CopyImage(file.clone()));
            add(&mut self.actions, &sub, "Show in Finder", RecentAction::Reveal(file.clone()));
            let _ = self.menu.append(&sub);
        }
        self.shown = entries;
        if cfg.save.enabled {
            if !self.shown.is_empty() {
                let _ = self.menu.append(&PredefinedMenuItem::separator());
            }
            add(&mut self.actions, &self.menu, "Open screenshots folder", RecentAction::OpenFolder);
        }
        self.menu.set_enabled(!self.shown.is_empty() || cfg.save.enabled);
    }

    /// Run the action behind a clicked menu item; false if the item isn't one of ours.
    fn click(&self, id: &MenuId) -> bool {
        let Some((_, action)) = self.actions.iter().find(|(item, _)| item == id) else { return false };
        match action {
            RecentAction::CopyImage(file) => {
                let file = file.clone();
                // Decoding a big PNG takes a moment; keep the menu bar responsive.
                std::thread::spawn(move || {
                    if let Err(e) = copy_image(&file) {
                        eprintln!("snipno: copying {} failed: {e}", file.display());
                    }
                });
            }
            RecentAction::Reveal(file) => open(&[std::ffi::OsStr::new("-R"), file.as_os_str()]),
            RecentAction::OpenFolder => {
                let folder = config::load().save.folder_path();
                if let Err(e) = std::fs::create_dir_all(&folder) {
                    eprintln!("snipno: could not create {}: {e}", folder.display());
                }
                open(&[folder.as_os_str()]);
            }
        }
        true
    }
}

fn add(actions: &mut Vec<(MenuId, RecentAction)>, menu: &Submenu, text: &str, action: RecentAction) {
    let item = MenuItem::new(text, true, None);
    if menu.append(&item).is_ok() {
        actions.push((item.id().clone(), action));
    }
}

fn copy_image(file: &Path) -> Result<(), Box<dyn Error>> {
    let image = xcap::image::open(file)?.to_rgba8();
    let (w, h) = image.dimensions();
    arboard::Clipboard::new()?.set_image(arboard::ImageData {
        width: w as usize,
        height: h as usize,
        bytes: std::borrow::Cow::Owned(image.into_raw()),
    })?;
    Ok(())
}

/// macOS `open`: a folder in Finder, or with `-R` a file selected in its folder.
fn open(args: &[&std::ffi::OsStr]) {
    if let Err(e) = Command::new("/usr/bin/open").args(args).status() {
        eprintln!("snipno: could not run open: {e}");
    }
}

pub fn run() -> Result<(), Box<dyn Error>> {
    config::drop_old_settings();
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

    let mut hotkeys = Hotkeys { manager: GlobalHotKeyManager::new()?, active: None };
    let capture_item = MenuItem::new("Take screenshot", true, None);
    hotkeys.apply_config(&capture_item);
    let mut recent = Recent::new();
    let settings_item = MenuItem::new("Settings…", true, None);
    let quit_item = MenuItem::new("Quit snipno", true, None);
    let menu = Menu::new();
    menu.append_items(&[
        &capture_item,
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
        // Wake up now and then to pick up new captures.
        *control_flow = ControlFlow::WaitUntil(Instant::now() + HISTORY_POLL);
        match event {
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => recent.refresh(false),

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
                    launch_capture(&capturing);
                }
            }

            Event::UserEvent(UserEvent::Menu(e)) => {
                if recent.click(&e.id) {
                    // Handled a "Recent screenshots" item.
                } else if e.id == *capture_item.id() {
                    launch_capture(&capturing);
                } else if e.id == *settings_item.id() {
                    // Already open (maybe lost behind other windows)? Replace it with a fresh one
                    // in front. Settings save immediately, so nothing is lost.
                    if let Some((_, mut old)) = settings.take() {
                        let _ = old.kill();
                        let _ = old.wait();
                        hotkeys.apply_config(&capture_item);
                        recent.refresh(true);
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
                    SettingsMsg::Resume => hotkeys.apply_config(&capture_item),
                    SettingsMsg::Reload => {
                        hotkeys.apply_config(&capture_item);
                        // Saving may have been switched on or off.
                        recent.refresh(true);
                    }
                    SettingsMsg::Closed => {
                        if let Some((_, mut child)) = settings.take() {
                            let _ = child.wait();
                        }
                        // Whatever happened in there, never leave the hotkey switched off.
                        hotkeys.apply_config(&capture_item);
                        recent.refresh(true);
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
/// overlay finishes (copied or cancelled: it prints [`MSG_CAPTURE_DONE`] or exits with code 0)
/// the others are closed. The finished one is left to save its screenshot.
fn launch_capture(capturing: &Arc<AtomicBool>) {
    // Ignore the hotkey while overlays are already up.
    if capturing.swap(true, Ordering::SeqCst) {
        return;
    }
    let exe = std::env::current_exe().unwrap_or_default();
    let mut children: Vec<(Child, Arc<AtomicBool>)> = monitor_ids_cursor_last()
        .into_iter()
        .filter_map(|id| {
            let mut cmd = Command::new(&exe);
            cmd.arg("capture").stdout(Stdio::piped());
            if let Some(id) = id {
                cmd.args(["--monitor", &id.to_string()]);
            }
            let mut child = cmd.spawn().map_err(|e| eprintln!("snipno: could not start capture: {e}")).ok()?;
            let done = Arc::new(AtomicBool::new(false));
            if let Some(stdout) = child.stdout.take() {
                let done = done.clone();
                std::thread::spawn(move || {
                    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                        if line.trim() == MSG_CAPTURE_DONE {
                            done.store(true, Ordering::SeqCst);
                        }
                    }
                });
            }
            Some((child, done))
        })
        .collect();

    let capturing = capturing.clone();
    std::thread::spawn(move || {
        let mut finished = None;
        'watch: while !children.is_empty() {
            let mut i = 0;
            while i < children.len() {
                if children[i].1.load(Ordering::SeqCst) {
                    finished = Some(children.remove(i).0);
                    break 'watch;
                }
                match children[i].0.try_wait() {
                    Ok(None) => i += 1,
                    Ok(Some(status)) if status.success() => {
                        finished = Some(children.remove(i).0);
                        break 'watch;
                    }
                    // An overlay that failed to start (e.g. a display that can't be captured)
                    // just drops out; the others stay up.
                    _ => {
                        let (mut failed, _) = children.remove(i);
                        let _ = failed.kill();
                        let _ = failed.wait();
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        for (child, _) in &mut children {
            let _ = child.kill();
            let _ = child.wait();
        }
        capturing.store(false, Ordering::SeqCst);
        if let Some(mut child) = finished {
            let _ = child.wait();
        }
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
}
