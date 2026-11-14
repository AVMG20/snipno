//! `snipno settings`: record the global hotkey and the overlay's tool shortcuts, and choose
//! whether and where captures are saved.
//! Tells the daemon over stdout to pause the hotkey while recording and to reload after a save
//! (see `daemon::MSG_*`).

use crate::{annotate::Tool, config, daemon, hotkey, icon, store, theme, toolbar};
use eframe::egui::{
    self, Color32, CornerRadius, Event, FontId, Key, Margin, RichText, Stroke, TextureHandle, vec2,
};
use std::{error::Error, sync::mpsc, time::Duration};

pub fn run() -> Result<(), Box<dyn Error>> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("snipno")
            .with_inner_size([460.0, 820.0])
            .with_resizable(false)
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false)
            .with_active(true),
        event_loop_builder: Some(Box::new(|builder| {
            #[cfg(target_os = "macos")]
            {
                use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
                builder
                    // Regular while open: a Dock icon and ⌘-Tab entry, so the window can't get
                    // lost behind other apps.
                    .with_activation_policy(ActivationPolicy::Regular)
                    .with_activate_ignoring_other_apps(true);
            }
            let _ = builder;
        })),
        centered: true,
        persist_window: false,
        ..Default::default()
    };
    eframe::run_native(
        "snipno-settings",
        options,
        Box::new(|cc| {
            theme::apply(&cc.egui_ctx);
            let logo = icon::app_icon(96);
            let logo = cc.egui_ctx.load_texture(
                "logo",
                egui::ColorImage::from_rgba_unmultiplied([96, 96], &icon::to_rgba(&logo)),
                egui::TextureOptions::LINEAR,
            );
            Ok(Box::new(Settings {
                config: config::load(),
                recording: None,
                tab: Tab::Shortcuts,
                status: None,
                picking: None,
                logo,
                snapshot: crate::snapshot::Snapshot::from_env(),
            }))
        }),
    )?;
    Ok(())
}

/// What the next key press will be recorded as.
#[derive(Clone, Copy, PartialEq)]
enum Target {
    Capture,
    Tool(Tool),
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Shortcuts,
    Saving,
}

const TABS: [(Tab, &str); 2] = [(Tab::Shortcuts, "Shortcuts"), (Tab::Saving, "Saving")];

struct Settings {
    config: config::Config,
    recording: Option<Target>,
    tab: Tab,
    status: Option<(String, bool)>,
    /// The folder picker is open; it sends the chosen folder, or `None` if cancelled.
    picking: Option<mpsc::Receiver<Option<String>>>,
    logo: TextureHandle,
    snapshot: Option<crate::snapshot::Snapshot>,
}

fn tell_daemon(msg: &str) {
    // stdout is line-buffered; when run by hand these lines are harmless.
    println!("{msg}");
}

impl Settings {
    fn start_recording(&mut self, target: Target) {
        // Pause the global hotkey for any recording, so pressing it here is recorded instead
        // of opening the overlay.
        if self.recording.is_none() {
            tell_daemon(daemon::MSG_PAUSE);
        }
        self.recording = Some(target);
        self.status = None;
    }

    fn stop_recording(&mut self) {
        if self.recording.take().is_some() {
            tell_daemon(daemon::MSG_RESUME);
        }
    }

    fn set_tool_key(&mut self, tool: Tool, key: String) {
        let clash = toolbar::TOOL_ORDER
            .into_iter()
            .find(|t| *t != tool && self.config.tool_keys.get(*t).eq_ignore_ascii_case(&key));
        if let Some(other) = clash {
            self.status = Some((format!("{} is already used for {}.", hotkey::pretty(&key), toolbar::tool_name(other)), false));
            return;
        }
        if key.eq_ignore_ascii_case(&self.config.hotkey) {
            self.status = Some((format!("{} is your capture shortcut.", hotkey::pretty(&key)), false));
            return;
        }
        self.config.tool_keys.set(tool, key);
        self.status = Some(match config::save(&self.config) {
            Ok(()) => ("Saved. Used from the next capture on.".to_string(), true),
            Err(e) => (format!("Could not save config: {e}"), false),
        });
    }

    /// Record the global capture hotkey.
    fn set_hotkey(&mut self, s: String) {
        if let Err(e) = hotkey::parse(&s) {
            self.status = Some((format!("Can't use that shortcut: {e}"), false));
            tell_daemon(daemon::MSG_RESUME);
            return;
        }
        self.config.hotkey = s;
        self.status = Some(match config::save(&self.config) {
            Ok(()) => {
                tell_daemon(daemon::MSG_RELOAD);
                ("Saved. The new shortcut works right away.".into(), true)
            }
            Err(e) => (format!("Could not save config: {e}"), false),
        });
    }

    /// Save after editing a saving setting; the daemon reloads to update the menu.
    fn save_and_reload(&mut self) {
        self.status = Some(match config::save(&self.config) {
            Ok(()) => {
                tell_daemon(daemon::MSG_RELOAD);
                ("Saved.".into(), true)
            }
            Err(e) => (format!("Could not save config: {e}"), false),
        });
    }

    fn record(&mut self, ctx: &egui::Context, target: Target) {
        let presses: Vec<_> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    Event::Key { key, physical_key, pressed: true, repeat: false, modifiers, .. } => {
                        Some((*key, physical_key.unwrap_or(*key), *modifiers))
                    }
                    _ => None,
                })
                .collect()
        });
        for (logical, physical, mods) in presses {
            if logical == Key::Escape && !mods.any() {
                self.stop_recording();
                self.status = None;
                return;
            }
            match target {
                // The global hotkey is registered by physical key position.
                Target::Capture => match hotkey::from_key_press(physical, mods) {
                    Some(s) => {
                        self.recording = None; // the reload below re-enables the hotkey
                        self.set_hotkey(s);
                        return;
                    }
                    None => {
                        self.status = Some((
                            format!("{} can't be used. Hold ⌘, ⌃ or ⌥ with a letter, digit or F-key.", physical.name()),
                            false,
                        ));
                    }
                },
                // Tool keys are matched by the character on the key, so `P` is P on any layout.
                Target::Tool(tool) => match hotkey::tool_key_from_press(logical, mods) {
                    Ok(s) => {
                        self.stop_recording();
                        self.set_tool_key(tool, s);
                        return;
                    }
                    Err(why) => self.status = Some((format!("{why}."), false)),
                },
            }
        }
    }

    fn header(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add(egui::Image::new((self.logo.id(), vec2(36.0, 36.0))));
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(RichText::new("snipno").size(16.0).color(theme::TEXT).strong());
                ui.label(RichText::new("Screenshots from your menu bar").size(12.0).color(theme::MUTED));
            });
        });
    }

    fn shortcut_card(&mut self, ui: &mut egui::Ui) {
        let target = Target::Capture;
        let (title, desc, current) =
            ("Capture shortcut", "Press it anywhere to start a capture.", self.config.hotkey.clone());
        egui::Frame::new()
            .fill(theme::SURFACE)
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::same(16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 3.0;
                        ui.label(RichText::new(title).size(13.5).color(theme::TEXT));
                        ui.label(RichText::new(desc).size(12.0).color(theme::MUTED));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.recording == Some(target) {
                            recording_pill(ui);
                        } else if current.is_empty() {
                            ui.label(RichText::new("None").size(12.5).color(theme::FAINT));
                        } else {
                            chips(ui, &current, 13.0);
                        }
                    });
                });

                ui.add_space(12.0);
                let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), egui::Sense::hover());
                ui.painter().rect_filled(line, 0.0, theme::BORDER);
                ui.add_space(12.0);

                ui.horizontal(|ui| {
                    if self.recording == Some(target) {
                        if ui.add(ghost_button("Cancel")).clicked() {
                            self.stop_recording();
                            self.status = None;
                        }
                        ui.label(RichText::new("Esc also cancels").size(12.0).color(theme::FAINT));
                    } else {
                        if ui.add(primary_button("Record new shortcut")).clicked() {
                            self.start_recording(target);
                        }
                        let is_default = current == config::DEFAULT_HOTKEY;
                        let reset = ui
                            .add_enabled(!is_default, ghost_button("Reset"))
                            .on_hover_text(format!("Back to {}", hotkey::pretty(config::DEFAULT_HOTKEY)));
                        if reset.clicked() {
                            self.set_hotkey(config::DEFAULT_HOTKEY.to_string());
                        }
                    }
                });
            });
    }

    fn tools_card(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            ui.label(RichText::new("Tool shortcuts").size(13.5).color(theme::TEXT));
            ui.label(RichText::new("Pick a tool while the capture overlay is open.").size(12.0).color(theme::MUTED));
            ui.add_space(8.0);
            ui.spacing_mut().item_spacing.y = 0.0;

            for tool in toolbar::TOOL_ORDER {
                separator(ui);
                ui.horizontal(|ui| {
                    ui.set_min_height(38.0);
                    ui.label(RichText::new(toolbar::tool_name(tool)).size(13.0).color(theme::TEXT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.recording == Some(Target::Tool(tool)) {
                            if ui.add(small_ghost_button("Cancel")).clicked() {
                                self.stop_recording();
                                self.status = None;
                            }
                            recording_pill(ui);
                        } else {
                            if ui.add(small_ghost_button("Change")).clicked() {
                                self.start_recording(Target::Tool(tool));
                            }
                            ui.add_space(4.0);
                            // Fixed-height box so the chips don't stretch to the row height.
                            let key = self.config.tool_keys.get(tool);
                            ui.allocate_ui_with_layout(
                                vec2(140.0, 24.0),
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| chips(ui, key, 12.5),
                            );
                        }
                    });
                });
            }

            separator(ui);
            ui.add_space(12.0);
            let defaults = config::ToolKeys::default();
            if ui
                .add_enabled(self.config.tool_keys != defaults, ghost_button("Reset to defaults"))
                .clicked()
            {
                self.stop_recording();
                self.config.tool_keys = defaults;
                self.status = Some(match config::save(&self.config) {
                    Ok(()) => ("Tool shortcuts reset.".into(), true),
                    Err(e) => (format!("Could not save config: {e}"), false),
                });
            }
        });
    }

    fn saving_card(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    ui.label(RichText::new("Save screenshots").size(13.5).color(theme::TEXT));
                    ui.label(
                        RichText::new("Keep a PNG of every capture, listed under Recent screenshots.")
                            .size(12.0)
                            .color(theme::MUTED),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if toggle(ui, &mut self.config.save.enabled).clicked() {
                        self.save_and_reload();
                    }
                });
            });
            ui.add_space(6.0);

            let enabled = self.config.save.enabled;
            let mut changed = false;
            ui.add_enabled_ui(enabled, |ui| {
                let save = &mut self.config.save;
                let default = config::tilde(&config::default_folder());
                changed |= field(ui, "Folder", &default, &mut save.folder);
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let choose = ui.add_enabled(self.picking.is_none(), small_ghost_button("Choose…"));
                    if choose.clicked() {
                        self.picking = Some(pick_folder(save.folder_path()));
                    }
                    if ui.add(small_ghost_button("Show in Finder")).clicked() {
                        let folder = save.folder_path();
                        let _ = std::fs::create_dir_all(&folder);
                        let _ = std::process::Command::new("/usr/bin/open").arg(&folder).spawn();
                    }
                    if !save.folder.trim().is_empty() && ui.add(small_ghost_button("Use default")).clicked() {
                        save.folder.clear();
                        changed = true;
                    }
                });

                changed |= field(ui, "File name", "{date}-{time}  ·  also {timestamp}", &mut save.name);
                ui.label(
                    RichText::new(format!("For example {}", store::file_name(&save.name, chrono::Local::now())))
                        .size(11.5)
                        .color(theme::FAINT),
                );

                ui.add_space(6.0);
                ui.label(RichText::new("Keep").size(12.0).color(theme::MUTED));
                ui.horizontal(|ui| {
                    let keep = ui.add(
                        egui::DragValue::new(&mut save.keep).range(0..=100_000).speed(1.0).update_while_editing(false),
                    );
                    // Save the number once it's set, not every step on the way there: a capture
                    // in between would clean up down to that step, and deleting can't be undone.
                    changed |= (keep.changed() && !keep.dragged()) || keep.drag_stopped();
                    let note = if save.keep == 0 {
                        "screenshots: all of them, nothing is deleted".to_string()
                    } else {
                        "newest screenshots, older ones are deleted (0 keeps all)".to_string()
                    };
                    ui.label(RichText::new(note).size(12.0).color(theme::MUTED));
                });
                ui.label(
                    RichText::new("Only files snipno saved are ever deleted, checked on each new capture.")
                        .size(11.5)
                        .color(theme::FAINT),
                );
            });
            if changed {
                // Captures read these when they start; the daemon's menu doesn't need a reload.
                self.save_config();
            }
        });
    }

    fn save_config(&mut self) {
        self.status = Some(match config::save(&self.config) {
            Ok(()) => ("Saved.".into(), true),
            Err(e) => (format!("Could not save config: {e}"), false),
        });
    }

    /// Take the folder chosen in the picker, once it's closed.
    fn poll_picker(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.picking else { return };
        match rx.try_recv() {
            Ok(choice) => {
                self.picking = None;
                if let Some(folder) = choice {
                    self.config.save.folder = folder;
                    self.save_config();
                }
            }
            Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(150)),
            Err(mpsc::TryRecvError::Disconnected) => self.picking = None,
        }
    }

    fn tabs(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(theme::SURFACE)
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(CornerRadius::same(9))
            .inner_margin(Margin::same(3))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.columns(TABS.len(), |cols| {
                    for (col, (tab, name)) in cols.iter_mut().zip(TABS) {
                        let on = self.tab == tab;
                        let button = egui::Button::new(
                            RichText::new(name).size(12.5).color(if on { theme::TEXT } else { theme::MUTED }),
                        )
                        .fill(if on { theme::SURFACE_HI } else { Color32::TRANSPARENT })
                        .stroke(if on { Stroke::new(1.0, theme::BORDER_HI) } else { Stroke::NONE })
                        .corner_radius(CornerRadius::same(7))
                        .min_size(vec2(col.available_width(), 26.0));
                        if col.vertical_centered_justified(|ui| ui.add(button)).inner.clicked() && !on {
                            self.stop_recording();
                            self.status = None;
                            self.tab = tab;
                        }
                    }
                });
            });
    }

    fn status(&self, ui: &mut egui::Ui) {
        let Some((msg, ok)) = &self.status else { return };
        ui.horizontal(|ui| {
            let color = if *ok { theme::SUCCESS } else { theme::DANGER };
            let (dot, _) = ui.allocate_exact_size(vec2(8.0, 14.0), egui::Sense::hover());
            ui.painter().circle_filled(dot.center(), 3.0, color);
            ui.label(RichText::new(msg).size(12.0).color(theme::MUTED));
        });
    }
}

fn card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(theme::SURFACE)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

/// A labelled single-line text field. Returns whether the text changed.
fn field(ui: &mut egui::Ui, label: &str, hint: &str, value: &mut String) -> bool {
    ui.add_space(6.0);
    ui.label(RichText::new(label).size(12.0).color(theme::MUTED));
    ui.add(
        egui::TextEdit::singleline(value)
            .hint_text(RichText::new(hint).color(theme::FAINT))
            .desired_width(f32::INFINITY)
            .font(FontId::proportional(13.0))
            .background_color(theme::SURFACE_HI)
            .margin(vec2(8.0, 5.0)),
    )
    .changed()
}

/// macOS's folder picker, run by `osascript` on its own thread. Sends the chosen folder (with
/// `~` for the home folder), or `None` when cancelled.
fn pick_folder(current: std::path::PathBuf) -> mpsc::Receiver<Option<String>> {
    let (tx, rx) = mpsc::channel();
    let start = if current.is_dir() { current } else { dirs::home_dir().unwrap_or_default() };
    std::thread::spawn(move || {
        let out = std::process::Command::new("/usr/bin/osascript")
            .args([
                "-e",
                "on run argv",
                "-e",
                "activate",
                "-e",
                "POSIX path of (choose folder with prompt \"Save screenshots in\" default location (POSIX file (item 1 of argv)))",
                "-e",
                "end run",
            ])
            .arg(&start)
            .output();
        let chosen = out.ok().filter(|o| o.status.success()).and_then(|o| {
            let path = String::from_utf8_lossy(&o.stdout).trim().trim_end_matches('/').to_string();
            (!path.is_empty()).then(|| config::tilde(std::path::Path::new(&path)))
        });
        let _ = tx.send(chosen);
    });
    rx
}

/// An on/off switch.
fn toggle(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(36.0, 20.0), egui::Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.12);
    let p = ui.painter();
    let bg = if *on { theme::ACCENT } else { theme::SURFACE_HI };
    p.rect_filled(rect, CornerRadius::same(255), bg);
    if !*on {
        p.rect_stroke(rect, CornerRadius::same(255), Stroke::new(1.0, theme::BORDER_HI), egui::StrokeKind::Inside);
    }
    let x = egui::lerp((rect.left() + 10.0)..=(rect.right() - 10.0), t);
    p.circle_filled(egui::pos2(x, rect.center().y), 7.0, Color32::WHITE);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn separator(ui: &mut egui::Ui) {
    let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().rect_filled(line, 0.0, theme::BORDER);
}

/// Key chips for a shortcut string, laid out right to left.
fn chips(ui: &mut egui::Ui, shortcut: &str, size: f32) {
    ui.spacing_mut().item_spacing.x = 4.0;
    for chip in theme::hotkey_chips(&hotkey::pretty(shortcut)).iter().rev() {
        theme::kbd(ui, chip, size);
    }
}

fn small_ghost_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).size(12.0))
        .fill(theme::SURFACE_HI)
        .stroke(Stroke::new(1.0, theme::BORDER_HI))
        .corner_radius(CornerRadius::same(7))
        .min_size(vec2(0.0, 26.0))
}

fn primary_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).color(Color32::WHITE).size(13.0))
        .fill(theme::ACCENT)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(8))
        .min_size(vec2(0.0, 30.0))
}

fn ghost_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).size(13.0))
        .fill(theme::SURFACE_HI)
        .stroke(Stroke::new(1.0, theme::BORDER_HI))
        .corner_radius(CornerRadius::same(8))
        .min_size(vec2(0.0, 30.0))
}

fn recording_pill(ui: &mut egui::Ui) {
    let t = ui.input(|i| i.time) as f32;
    let pulse = 0.55 + 0.45 * (t * 4.0).sin().abs();
    egui::Frame::new()
        .fill(theme::ACCENT_SOFT)
        .stroke(Stroke::new(1.0, theme::ACCENT))
        .corner_radius(CornerRadius::same(255))
        .inner_margin(Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (dot, _) = ui.allocate_exact_size(vec2(6.0, 14.0), egui::Sense::hover());
                ui.painter().circle_filled(dot.center(), 3.0, theme::ACCENT.gamma_multiply(pulse));
                ui.label(RichText::new("Press keys…").font(FontId::proportional(12.5)).color(theme::ACCENT));
            });
        });
    ui.ctx().request_repaint();
}

impl eframe::App for Settings {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if let Some(snap) = &mut self.snapshot {
            snap.tick(&ctx);
        }
        self.poll_picker(&ctx);
        if let Some(target) = self.recording {
            self.record(&ctx, target);
        } else if ctx.input(|i| i.modifiers.command && i.key_pressed(Key::W))
            || (!ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_pressed(Key::Escape)))
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(Margin { left: 24, right: 24, top: 40, bottom: 18 }))
            .show(ui, |ui| {
                self.header(ui);
                ui.add_space(16.0);
                self.tabs(ui);
                ui.add_space(12.0);
                match self.tab {
                    Tab::Shortcuts => {
                        self.shortcut_card(ui);
                        ui.add_space(12.0);
                        self.tools_card(ui);
                    }
                    Tab::Saving => self.saving_card(ui),
                }
                ui.add_space(10.0);
                self.status(ui);

                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    let path = config::path().display().to_string().replace(
                        &dirs::home_dir().map(|h| h.display().to_string()).unwrap_or_default(),
                        "~",
                    );
                    ui.label(RichText::new(path).font(FontId::monospace(11.0)).color(theme::FAINT));
                });
            });
    }
}
