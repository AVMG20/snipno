//! `snipno settings`: record the global capture hotkey and the overlay's tool shortcuts.
//! Tells the daemon over stdout to pause the hotkey while recording and to reload after a save
//! (see `daemon::MSG_*`).

use crate::{annotate::Tool, config, daemon, hotkey, icon, theme, toolbar};
use eframe::egui::{
    self, Color32, CornerRadius, Event, FontId, Key, Margin, RichText, Stroke, TextureHandle, vec2,
};
use std::error::Error;

pub fn run() -> Result<(), Box<dyn Error>> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("snipno")
            .with_inner_size([460.0, 610.0])
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
                status: None,
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

struct Settings {
    config: config::Config,
    recording: Option<Target>,
    status: Option<(String, bool)>,
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
                        ui.label(RichText::new("Capture shortcut").size(13.5).color(theme::TEXT));
                        ui.label(RichText::new("Press it anywhere to start a capture.").size(12.0).color(theme::MUTED));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.recording == Some(Target::Capture) {
                            recording_pill(ui);
                        } else {
                            chips(ui, &self.config.hotkey, 13.0);
                        }
                    });
                });

                ui.add_space(12.0);
                let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), egui::Sense::hover());
                ui.painter().rect_filled(line, 0.0, theme::BORDER);
                ui.add_space(12.0);

                ui.horizontal(|ui| {
                    if self.recording == Some(Target::Capture) {
                        if ui.add(ghost_button("Cancel")).clicked() {
                            self.stop_recording();
                            self.status = None;
                        }
                        ui.label(RichText::new("Esc also cancels").size(12.0).color(theme::FAINT));
                    } else {
                        if ui.add(primary_button("Record new shortcut")).clicked() {
                            self.start_recording(Target::Capture);
                        }
                        let is_default = self.config.hotkey == config::DEFAULT_HOTKEY;
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
                .add_enabled(self.config.tool_keys != defaults, ghost_button("Reset to S P A R E"))
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
        if let Some(target) = self.recording {
            self.record(&ctx, target);
        } else if ctx.input(|i| i.key_pressed(Key::Escape) || (i.modifiers.command && i.key_pressed(Key::W))) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(Margin { left: 24, right: 24, top: 40, bottom: 18 }))
            .show(ui, |ui| {
                self.header(ui);
                ui.add_space(18.0);
                self.shortcut_card(ui);
                ui.add_space(12.0);
                self.tools_card(ui);
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
